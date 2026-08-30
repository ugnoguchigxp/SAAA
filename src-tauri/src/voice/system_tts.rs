use std::{
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

#[cfg(not(target_os = "macos"))]
use std::env;

use crate::{AppState, RunCancellation};

#[cfg(target_os = "macos")]
#[path = "audio_output.rs"]
pub(crate) mod audio_output;
#[cfg(target_os = "macos")]
use audio_output::RenderedTtsAudio;

pub(crate) struct SpawnedTts {
    pub(crate) child: Child,
    #[cfg(target_os = "macos")]
    pub(crate) rendered_audio: RenderedTtsAudio,
}

pub(crate) fn prepare_output(state: &AppState, run_id: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        state.tts_audio_output.prepare(run_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (state, run_id);
        Ok(())
    }
}

pub(crate) fn cancel_output(state: &AppState, run_id: &str) {
    #[cfg(target_os = "macos")]
    state.tts_audio_output.cancel(run_id);
    #[cfg(not(target_os = "macos"))]
    let _ = (state, run_id);
}

#[cfg(target_os = "macos")]
pub(crate) fn spawn_tts_process(text: &str, voice: &str) -> Result<SpawnedTts, String> {
    let rendered_audio = RenderedTtsAudio::new()?;
    let mut command = Command::new("say");
    if voice != "default" {
        command.arg("-v").arg(voice);
    }
    command
        .arg("-o")
        .arg(rendered_audio.path())
        .arg("--file-format=WAVE")
        .arg("--data-format=LEI16@22050")
        .arg(text)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn().map_err(spawn_error)?;
    Ok(SpawnedTts {
        child,
        rendered_audio,
    })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn spawn_tts_process(text: &str, voice: &str) -> Result<SpawnedTts, String> {
    let mut command = match env::consts::OS {
        "linux" => {
            let mut command = Command::new("espeak-ng");
            if voice != "default" {
                command.arg("-v").arg(voice);
            }
            command.arg(text);
            command
        }
        "windows" => {
            let mut command = Command::new("powershell.exe");
            command
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; if ($env:SAAA_TTS_VOICE -ne 'default') { $s.SelectVoice($env:SAAA_TTS_VOICE) }; $s.Speak($env:SAAA_TTS_TEXT)",
                ])
                .env("SAAA_TTS_TEXT", text)
                .env("SAAA_TTS_VOICE", voice);
            command
        }
        _ => return Err("System TTS is not supported on this platform".to_string()),
    };
    let child = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(spawn_error)?;
    Ok(SpawnedTts { child })
}

pub(crate) async fn complete_output(
    state: &AppState,
    run_id: &str,
    cancellation: Arc<RunCancellation>,
    synthesis_result: Result<(), String>,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let result = async {
            synthesis_result?;
            let audio = {
                let mut process = state
                    .tts_process
                    .lock()
                    .map_err(|_| "TTS process lock unavailable".to_string())?;
                process
                    .as_mut()
                    .filter(|active| active.run_id == run_id)
                    .and_then(|active| active.rendered_audio.take())
                    .ok_or_else(|| "Rendered TTS audio was unavailable".to_string())?
            };
            let completion = state.tts_audio_output.play(run_id, audio)?;
            loop {
                if cancellation.is_cancelled() {
                    return Err("Speech cancelled".to_string());
                }
                match completion.try_recv() {
                    Ok(result) => return result,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        return Err("TTS audio output stopped during playback".to_string());
                    }
                }
            }
        }
        .await;
        if result.is_err() {
            state.tts_audio_output.cancel(run_id);
        }
        result
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (state, run_id, cancellation);
        synthesis_result
    }
}

fn spawn_error(error: std::io::Error) -> String {
    format!(
        "Could not start local system TTS: {error}. Install the platform speech runtime and retry."
    )
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::spawn_tts_process;

    #[test]
    fn renders_wave_audio_without_opening_the_output_device() {
        let mut spawned =
            spawn_tts_process("[[slnc 10]]", "default").expect("system TTS process starts");
        assert!(spawned
            .child
            .wait()
            .expect("system TTS process completes")
            .success());
        let bytes = std::fs::read(spawned.rendered_audio.path()).expect("rendered WAV is readable");
        assert!(bytes.starts_with(b"RIFF"));
        assert_eq!(bytes.get(8..12), Some(b"WAVE".as_slice()));
    }
}
