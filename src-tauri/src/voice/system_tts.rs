use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, OnceLock},
};

use serde::Deserialize;

use crate::RunCancellation;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlatformVoice {
    id: String,
}

pub(crate) fn validate_voice(voice: &str) -> Result<(), String> {
    if voice == "default" {
        return Ok(());
    }
    if cached_platform_voices().is_ok_and(|voices| voices.iter().any(|item| item.id == voice)) {
        Ok(())
    } else {
        Err(format!("System TTS voice is unavailable: {voice}"))
    }
}

fn cached_platform_voices() -> Result<Vec<PlatformVoice>, String> {
    static VOICES: OnceLock<Result<Vec<PlatformVoice>, String>> = OnceLock::new();
    VOICES.get_or_init(platform_voices).clone()
}

pub(crate) async fn render_tts_artifact(
    text: String,
    voice: String,
    directory: PathBuf,
    cancellation: Arc<RunCancellation>,
) -> Result<PathBuf, String> {
    use tokio::io::AsyncWriteExt;

    validate_voice(&voice)?;
    prepare_render_directory(&directory)?;
    let path = directory.join(format!("tts-{}.wav", uuid::Uuid::new_v4()));
    if fs::symlink_metadata(&path).is_ok() {
        return Err("System TTS artifact already exists".to_string());
    }
    let mut command = render_command(&voice, &path)?;
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start the system TTS renderer".to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "System TTS renderer input pipe was unavailable".to_string())?;
    if stdin.write_all(text.as_bytes()).await.is_err() || stdin.shutdown().await.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        let _ = fs::remove_file(&path);
        return Err("Could not send text to the system TTS renderer".to_string());
    }
    let status = tokio::select! {
        _ = cancellation.cancelled() => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = fs::remove_file(&path);
            return Err("Speech cancelled".to_string());
        }
        status = child.wait() => status.map_err(|_| "System TTS renderer stopped unexpectedly".to_string())?,
    };
    if !status.success() {
        let _ = fs::remove_file(&path);
        return Err(format!("System TTS renderer exited with {status}"));
    }
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| "System TTS renderer did not create an audio artifact".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() < 12 {
        let _ = fs::remove_file(&path);
        return Err("System TTS renderer created an invalid audio artifact".to_string());
    }
    #[cfg(unix)]
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|_| "Could not secure the system TTS artifact".to_string())?;
    Ok(path)
}

fn prepare_render_directory(directory: &Path) -> Result<(), String> {
    if directory.exists() {
        let metadata = fs::symlink_metadata(directory)
            .map_err(|_| "Could not inspect the system TTS cache".to_string())?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("System TTS cache must be a private directory".to_string());
        }
    } else {
        fs::create_dir_all(directory)
            .map_err(|_| "Could not create the system TTS cache".to_string())?;
    }
    #[cfg(unix)]
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Could not secure the system TTS cache".to_string())?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn render_command(voice: &str, path: &Path) -> Result<tokio::process::Command, String> {
    let mut command = tokio::process::Command::new("say");
    if voice != "default" {
        command.arg("-v").arg(voice);
    }
    command
        .arg("-o")
        .arg(path)
        .args(["--file-format=WAVE", "--data-format=LEI16@22050"]);
    Ok(command)
}

#[cfg(target_os = "linux")]
fn render_command(voice: &str, path: &Path) -> Result<tokio::process::Command, String> {
    let mut command = tokio::process::Command::new("espeak-ng");
    if voice != "default" {
        command.arg("-v").arg(voice);
    }
    command.arg("-w").arg(path).arg("--stdin");
    Ok(command)
}

#[cfg(target_os = "windows")]
fn render_command(voice: &str, path: &Path) -> Result<tokio::process::Command, String> {
    let mut command = tokio::process::Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Add-Type -AssemblyName System.Speech; $s=New-Object System.Speech.Synthesis.SpeechSynthesizer; if($env:SAAA_TTS_VOICE -ne 'default'){$s.SelectVoice($env:SAAA_TTS_VOICE)}; $s.SetOutputToWaveFile($env:SAAA_TTS_PATH); $s.Speak([Console]::In.ReadToEnd()); $s.Dispose()",
        ])
        .env("SAAA_TTS_VOICE", voice)
        .env("SAAA_TTS_PATH", path);
    Ok(command)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn render_command(_voice: &str, _path: &Path) -> Result<tokio::process::Command, String> {
    Err("System TTS rendering is not supported on this platform".to_string())
}

#[cfg(target_os = "macos")]
fn platform_voices() -> Result<Vec<PlatformVoice>, String> {
    let output = Command::new("say")
        .args(["-v", "?"])
        .output()
        .map_err(spawn_error)?;
    if !output.status.success() || output.stdout.len() > 256 * 1_024 {
        return Err("Could not enumerate macOS system voices".to_string());
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "macOS voice list was not UTF-8".to_string())?;
    let voices = text
        .lines()
        .filter_map(parse_macos_voice)
        .collect::<Vec<_>>();
    if voices.is_empty() {
        Err("No macOS system voices are installed".to_string())
    } else {
        Ok(voices)
    }
}

#[cfg(target_os = "macos")]
fn parse_macos_voice(line: &str) -> Option<PlatformVoice> {
    let marker = line.find(" #")?;
    let identity = line[..marker].trim_end();
    let language_start = identity.rfind(char::is_whitespace)? + 1;
    let language = identity[language_start..].trim();
    let id = identity[..language_start].trim();
    if id.is_empty() || language.len() < 2 {
        return None;
    }
    Some(PlatformVoice { id: id.to_string() })
}

#[cfg(target_os = "linux")]
fn platform_voices() -> Result<Vec<PlatformVoice>, String> {
    let output = Command::new("espeak-ng")
        .arg("--voices")
        .output()
        .map_err(spawn_error)?;
    if !output.status.success() || output.stdout.len() > 256 * 1_024 {
        return Err("Could not enumerate espeak-ng voices".to_string());
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| "espeak-ng voice list was not UTF-8".to_string())?;
    let voices = text
        .lines()
        .skip(1)
        .filter_map(|line| {
            let columns = line.split_whitespace().collect::<Vec<_>>();
            columns.get(1)?;
            let id = columns.get(3).or_else(|| columns.get(1))?;
            Some(PlatformVoice {
                id: (*id).to_string(),
            })
        })
        .collect::<Vec<_>>();
    if voices.is_empty() {
        Err("No espeak-ng voices are installed".to_string())
    } else {
        Ok(voices)
    }
}

#[cfg(target_os = "windows")]
fn platform_voices() -> Result<Vec<PlatformVoice>, String> {
    let output = Command::new("powershell.exe").args([
        "-NoProfile", "-NonInteractive", "-Command",
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Add-Type -AssemblyName System.Speech; $s=New-Object System.Speech.Synthesis.SpeechSynthesizer; @($s.GetInstalledVoices()|%{$_.VoiceInfo}|%{@{id=$_.Name;label=$_.Description;language=$_.Culture.Name}})|ConvertTo-Json -Compress",
    ]).output().map_err(spawn_error)?;
    if !output.status.success() || output.stdout.len() > 256 * 1_024 {
        return Err("Could not enumerate Windows system voices".to_string());
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "Windows voice list was invalid".to_string())?;
    match value {
        serde_json::Value::Array(_) => serde_json::from_value(value),
        serde_json::Value::Object(_) => serde_json::from_value(value).map(|voice| vec![voice]),
        _ => return Err("Windows voice list was invalid".to_string()),
    }
    .map_err(|_| "Windows voice list was invalid".to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn platform_voices() -> Result<Vec<PlatformVoice>, String> {
    Err("System TTS is not supported on this platform".to_string())
}

fn spawn_error(error: std::io::Error) -> String {
    format!(
        "Could not start local system TTS: {error}. Install the platform speech runtime and retry."
    )
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn parses_multiword_macos_voice_names() {
        let voice = parse_macos_voice("Grandma (German (Germany))  de_DE    # Hallo!").unwrap();
        assert_eq!(voice.id, "Grandma (German (Germany))");
    }
}
