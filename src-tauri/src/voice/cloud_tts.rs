use futures_util::StreamExt;
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

use crate::{CloudTtsProviderSettings, RunCancellation};
use zeroize::Zeroizing;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const MAX_AUDIO_BYTES: usize = 16 * 1_024 * 1_024;

pub(crate) async fn probe(provider: &CloudTtsProviderSettings) -> Result<String, String> {
    let cancellation = Arc::new(RunCancellation::default());
    let audio = synthesize(provider, "Connectivity check", 10_000, cancellation).await?;
    if audio.is_empty() {
        return Err("Cloud TTS returned empty audio".to_string());
    }
    Ok("Cloud TTS generated a bounded audio preview".to_string())
}

pub(crate) async fn synthesize_to_player(
    provider: &CloudTtsProviderSettings,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    directory: &Path,
) -> Result<(Child, PathBuf), String> {
    let audio = synthesize(provider, text, timeout_ms, cancellation).await?;
    prepare_cache_directory(directory)?;
    let path = directory.join(format!("tts-{}.wav", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&path)
        .map_err(|_| "Could not create the TTS audio artifact".to_string())?;
    if file
        .write_all(&audio)
        .and_then(|_| file.sync_all())
        .is_err()
    {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err("Could not write the TTS audio artifact".to_string());
    }
    match spawn_player(&path) {
        Ok(child) => Ok((child, path)),
        Err(error) => {
            let _ = fs::remove_file(&path);
            Err(error)
        }
    }
}

pub(crate) fn cleanup_cache(directory: &Path) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    prepare_cache_directory(directory)?;
    for entry in fs::read_dir(directory)
        .map_err(|_| "Could not inspect the TTS cache directory".to_string())?
    {
        let entry = entry.map_err(|_| "Could not inspect a TTS cache artifact".to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with("tts-")
            && name.ends_with(".wav")
            && entry
                .file_type()
                .map_err(|_| "Could not inspect a TTS cache artifact".to_string())?
                .is_file()
        {
            fs::remove_file(entry.path())
                .map_err(|_| "Could not remove a stale TTS cache artifact".to_string())?;
        }
    }
    Ok(())
}

fn prepare_cache_directory(directory: &Path) -> Result<(), String> {
    if directory.exists() {
        let metadata = fs::symlink_metadata(directory)
            .map_err(|_| "Could not inspect the TTS cache directory".to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("TTS cache path must be a private directory".to_string());
        }
    } else {
        fs::create_dir_all(directory)
            .map_err(|_| "Could not create the TTS cache directory".to_string())?;
    }
    #[cfg(unix)]
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Could not secure the TTS cache directory".to_string())?;
    Ok(())
}

async fn synthesize(
    provider: &CloudTtsProviderSettings,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_millis(timeout_ms))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize the Cloud TTS client".to_string())?;
    let api_key = credential(provider)?;
    let mut request = client
        .post(operation_url(&provider.endpoint)?)
        .json(&json!({
            "model": provider.model,
            "input": text,
            "voice": provider.voice,
            "response_format": "wav"
        }));
    if let Some(api_key) = api_key.as_deref() {
        request = request.bearer_auth(api_key.as_str());
    }
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err("Speech cancelled".to_string()),
        response = request.send() => response.map_err(|error| {
            if error.is_timeout() { "Cloud TTS request timed out" } else { "Cloud TTS request failed" }.to_string()
        })?,
    };
    if !response.status().is_success() {
        return Err(format!("Cloud TTS returned HTTP {}", response.status()));
    }
    bounded_audio(response, &cancellation).await
}

fn credential(
    provider: &CloudTtsProviderSettings,
) -> Result<Option<zeroize::Zeroizing<String>>, String> {
    if provider.authentication == "none" {
        return Ok(None);
    }
    crate::credentials::load_api_key(&provider.id)?
        .ok_or_else(|| "API key is not configured in macOS Keychain".to_string())
        .map(Some)
}

fn operation_url(endpoint: &str) -> Result<String, String> {
    let mut url =
        url::Url::parse(endpoint).map_err(|_| "Cloud TTS endpoint is invalid".to_string())?;
    let mut path = url.path().trim_end_matches('/').to_string();
    if !path.ends_with("/v1") {
        path.push_str("/v1");
    }
    path.push_str("/audio/speech");
    url.set_path(&path);
    Ok(url.to_string())
}

async fn bounded_audio(
    response: reqwest::Response,
    cancellation: &RunCancellation,
) -> Result<Zeroizing<Vec<u8>>, String> {
    if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let content_type = content_type
            .to_str()
            .map_err(|_| "Cloud TTS returned an invalid content type".to_string())?;
        if !is_audio_content_type(content_type) {
            return Err("Cloud TTS returned a non-audio response".to_string());
        }
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AUDIO_BYTES as u64)
    {
        return Err("Cloud TTS audio exceeded the size limit".to_string());
    }
    let mut stream = response.bytes_stream();
    let mut audio = Zeroizing::new(Vec::new());
    loop {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err("Speech cancelled".to_string()),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| "Cloud TTS response was interrupted".to_string())?;
        if audio.len().saturating_add(chunk.len()) > MAX_AUDIO_BYTES {
            return Err("Cloud TTS audio exceeded the size limit".to_string());
        }
        audio.extend_from_slice(&chunk);
    }
    if !is_wav(&audio) {
        return Err("Cloud TTS did not return valid WAV audio".to_string());
    }
    Ok(audio)
}

fn is_audio_content_type(value: &str) -> bool {
    let media_type = value.split(';').next().unwrap_or_default().trim();
    media_type.starts_with("audio/") || media_type == "application/octet-stream"
}

fn is_wav(audio: &[u8]) -> bool {
    audio.len() >= 12 && &audio[..4] == b"RIFF" && &audio[8..12] == b"WAVE"
}

#[cfg(target_os = "macos")]
fn spawn_player(path: &Path) -> Result<Child, String> {
    Command::new("afplay")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start macOS audio playback".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_wav_audio_responses_are_accepted() {
        assert!(is_audio_content_type("audio/wav; charset=binary"));
        assert!(is_audio_content_type("application/octet-stream"));
        assert!(!is_audio_content_type("application/json"));
        assert!(is_wav(b"RIFF\x04\x00\x00\x00WAVE"));
        assert!(!is_wav(br#"{\"error\":\"upstream\"}"#));
    }

    #[test]
    fn startup_cleanup_removes_only_owned_tts_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("tts-stale.wav"), b"audio").unwrap();
        std::fs::write(directory.path().join("keep.txt"), b"keep").unwrap();
        cleanup_cache(directory.path()).unwrap();
        assert!(!directory.path().join("tts-stale.wav").exists());
        assert!(directory.path().join("keep.txt").exists());
    }
}

#[cfg(target_os = "linux")]
fn spawn_player(path: &Path) -> Result<Child, String> {
    Command::new("aplay")
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start audio playback".to_string())
}

#[cfg(target_os = "windows")]
fn spawn_player(path: &Path) -> Result<Child, String> {
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(New-Object Media.SoundPlayer $args[0]).PlaySync()",
        ])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Could not start audio playback".to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn spawn_player(_path: &Path) -> Result<Child, String> {
    Err("Cloud TTS playback is not supported on this platform".to_string())
}
