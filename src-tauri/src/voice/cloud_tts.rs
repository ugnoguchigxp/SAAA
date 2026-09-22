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

pub(crate) async fn render_to_artifact(
    provider: &CloudTtsProviderSettings,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    directory: &Path,
) -> Result<PathBuf, String> {
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
    Ok(path)
}

pub(crate) fn spawn_audio_player(path: &Path) -> Result<Child, String> {
    spawn_player(path)
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

pub(crate) async fn synthesize(
    provider: &CloudTtsProviderSettings,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let response = request_audio(provider, text, timeout_ms, cancellation.clone()).await?;
    validate_audio_headers(&response, &provider.response_format)?;
    let audio = bounded_audio(response, &cancellation, &provider.response_format).await?;
    if provider.response_format == "pcm" {
        let mut wav = Zeroizing::new(Vec::with_capacity(44 + audio.len()));
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + audio.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&24_000_u32.to_le_bytes());
        wav.extend_from_slice(&48_000_u32.to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(audio.len() as u32).to_le_bytes());
        wav.extend_from_slice(&audio);
        return Ok(wav);
    }
    Ok(audio)
}

pub(crate) async fn request_audio(
    provider: &CloudTtsProviderSettings,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<reqwest::Response, String> {
    request_audio_with_api_key(provider, text, timeout_ms, cancellation, None).await
}

pub(crate) async fn request_audio_with_api_key(
    provider: &CloudTtsProviderSettings,
    text: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    claim_key: Option<&str>,
) -> Result<reqwest::Response, String> {
    let client = super::http_audio::client::build(
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_millis(timeout_ms))
            .redirect(reqwest::redirect::Policy::none()),
        claim_key.is_some(),
    )?;
    let configured_key = if claim_key.is_none() {
        credential(provider)?
    } else {
        None
    };
    let api_key = claim_key.or(configured_key.as_deref().map(String::as_str));
    let mut request = client
        .post(operation_url(&provider.endpoint)?)
        .json(&json!({
            "model": provider.model,
            "input": text,
            "voice": provider.voice,
            "response_format": provider.response_format
        }));
    if let Some(api_key) = api_key {
        request = request.bearer_auth(api_key);
    }
    let response = crate::providers::http::send(request, &cancellation, true)
        .await
        .map_err(|kind| kind.public_message().as_str().to_string())?;
    Ok(response)
}

fn credential(
    provider: &CloudTtsProviderSettings,
) -> Result<Option<zeroize::Zeroizing<String>>, String> {
    if provider.authentication == "none" {
        return Ok(None);
    }
    crate::credentials::load_api_key(&provider.id)?
        .ok_or_else(|| {
            "API key is not configured in the operating system credential store".to_string()
        })
        .map(Some)
}

fn operation_url(endpoint: &str) -> Result<String, String> {
    crate::providers::openai_compatible::provider_operation_url(endpoint, "audio/speech")
}

pub(crate) fn validate_audio_headers(
    response: &reqwest::Response,
    format: &str,
) -> Result<(), String> {
    if let Some(value) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let media = value
            .to_str()
            .map_err(|_| "Invalid audio Content-Type".to_string())?
            .split(';')
            .next()
            .unwrap_or_default()
            .trim();
        let accepted = match format {
            "wav" => matches!(
                media,
                "audio/wav" | "audio/x-wav" | "audio/wave" | "application/octet-stream"
            ),
            "pcm" => matches!(media, "audio/pcm" | "application/octet-stream"),
            _ => false,
        };
        if !accepted {
            return Err("TTS response Content-Type disagrees with the requested format".into());
        }
    }
    if response
        .content_length()
        .is_some_and(|bytes| bytes > MAX_AUDIO_BYTES as u64)
    {
        return Err("TTS audio exceeded the size limit".into());
    }
    Ok(())
}

async fn bounded_audio(
    response: reqwest::Response,
    cancellation: &RunCancellation,
    format: &str,
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
    let mut decoder = crate::voice::http_audio::decode::Decoder::new(format)?;
    decoder.push(&audio)?;
    decoder.finish()?;
    Ok(audio)
}

fn is_audio_content_type(value: &str) -> bool {
    let media_type = value.split(';').next().unwrap_or_default().trim();
    media_type.starts_with("audio/") || media_type == "application/octet-stream"
}

#[cfg(test)]
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
