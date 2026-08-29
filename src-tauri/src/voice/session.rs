use std::env;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

use crate::redact::redact_runtime_text;
use crate::{
    begin_simple_runtime_run, finish_runtime_run, register_active_run, remove_active_run,
    validate_identifier, ActiveTts, AppState, PreviewAudioInput, RunCancellation, SpeakTextInput,
    TranscribeAudioInput, VoiceEvent,
};

pub(crate) async fn transcribe_audio(
    state: &AppState,
    mut input: TranscribeAudioInput,
    on_event: tauri::ipc::Channel<VoiceEvent>,
) -> Result<String, String> {
    validate_identifier(&input.run_id, "run id")?;
    validate_identifier(&input.conversation_id, "conversation id")?;
    if !(8_000..=192_000).contains(&input.sample_rate) || input.samples.is_empty() {
        return Err("Recorded audio is empty or has an unsupported sample rate".to_string());
    }
    if input.samples.iter().any(|sample| !sample.is_finite()) {
        return Err("Recorded audio contains invalid samples".to_string());
    }
    if input.samples.len() > input.sample_rate as usize * 300 {
        return Err("Recording exceeds the five minute MVP limit".to_string());
    }
    if input.model != crate::voice::network_asr::MODEL_ID {
        return Err("Voice settings must use the configured LAN ASR model".to_string());
    }
    let samples = Zeroizing::new(std::mem::take(&mut input.samples));
    let verification = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        state
            .voice_profile
            .verify_if_enabled(&connection, &samples, input.sample_rate)
    };
    if let Err(error) = verification {
        let recovery = "Use the enrolled microphone and speak clearly, or disable the target-speaker filter in Settings.".to_string();
        let _ = on_event.send(VoiceEvent::Failed {
            run_id: input.run_id.clone(),
            message: error.clone(),
            recovery,
        });
        return Err(error);
    }
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(state, &input.run_id, cancellation.clone())?;
    if let Err(error) = begin_simple_runtime_run(
        state,
        &input.run_id,
        &input.conversation_id,
        "voice.transcribe",
        crate::voice::network_asr::PROVIDER_ID,
    ) {
        remove_active_run(state, &input.run_id);
        state
            .situation
            .set_microphone_state(crate::situation::contracts::MicrophoneState::Inactive);
        return Err(error);
    }
    let _ = on_event.send(VoiceEvent::Transcribing {
        run_id: input.run_id.clone(),
    });
    state
        .situation
        .set_microphone_state(crate::situation::contracts::MicrophoneState::SaaaTranscribing);
    let result = crate::voice::network_asr::transcribe(
        &samples,
        input.sample_rate,
        &input.model,
        cancellation.clone(),
    )
    .await
    .map(|(text, _language)| text);
    remove_active_run(state, &input.run_id);
    state
        .situation
        .set_microphone_state(crate::situation::contracts::MicrophoneState::Inactive);
    match result {
        Ok(transcript) => {
            finish_runtime_run(state, &input.run_id, "completed", None)?;
            let _ = on_event.send(VoiceEvent::TranscriptFinal {
                run_id: input.run_id,
                text: transcript.clone(),
            });
            Ok(transcript)
        }
        Err(error) if cancellation.is_cancelled() => {
            finish_runtime_run(state, &input.run_id, "cancelled", Some("Cancelled by user"))?;
            let _ = on_event.send(VoiceEvent::Cancelled {
                run_id: input.run_id,
            });
            Err("Transcription cancelled".to_string())
        }
        Err(error) => {
            let error = redact_runtime_text(&error);
            finish_runtime_run(state, &input.run_id, "failed", Some(&error))?;
            let _ = on_event.send(VoiceEvent::Failed {
                run_id: input.run_id,
                message: error.clone(),
                recovery: "Check the LAN ASR service and retry.".to_string(),
            });
            Err(error)
        }
    }
}

pub(crate) async fn preview_audio(
    state: &AppState,
    mut input: PreviewAudioInput,
    on_event: tauri::ipc::Channel<VoiceEvent>,
) -> Result<String, String> {
    validate_identifier(&input.run_id, "run id")?;
    validate_identifier(&input.conversation_id, "conversation id")?;
    if !(8_000..=192_000).contains(&input.sample_rate)
        || input.samples.len() < input.sample_rate as usize
        || input.samples.len() > input.sample_rate as usize * 15
        || input.samples.iter().any(|sample| !sample.is_finite())
    {
        return Err(
            "Voice preview must contain between one and fifteen seconds of valid audio".to_string(),
        );
    }
    if input.model != crate::voice::network_asr::MODEL_ID {
        return Err("Voice settings must use the configured LAN ASR model".to_string());
    }
    let samples = Zeroizing::new(std::mem::take(&mut input.samples));
    {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        state
            .voice_profile
            .verify_if_enabled(&connection, &samples, input.sample_rate)?;
    }
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(state, &input.run_id, cancellation.clone())?;
    let result = crate::voice::network_asr::transcribe(
        &samples,
        input.sample_rate,
        &input.model,
        cancellation.clone(),
    )
    .await
    .map(|(text, _language)| text);
    remove_active_run(state, &input.run_id);
    match result {
        Ok(transcript) => {
            let _ = on_event.send(VoiceEvent::TranscriptDelta {
                run_id: input.run_id,
                text: transcript.clone(),
            });
            Ok(transcript)
        }
        Err(_) if cancellation.is_cancelled() => Err("Voice preview cancelled".to_string()),
        Err(error) => Err(redact_runtime_text(&error)),
    }
}

pub(crate) async fn speak_text(state: &AppState, input: SpeakTextInput) -> Result<(), String> {
    validate_identifier(&input.run_id, "run id")?;
    validate_identifier(&input.conversation_id, "conversation id")?;
    if input.text.trim().is_empty() || input.text.chars().count() > 16_000 {
        return Err("Speech text must contain between 1 and 16,000 characters".to_string());
    }
    if input.voice.trim().is_empty() || input.voice.len() > 160 {
        return Err("TTS voice must contain between 1 and 160 characters".to_string());
    }
    let speech_text = crate::voice::tts::text_for_speech(&input.text);
    if speech_text.is_empty() {
        return Ok(());
    }
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(state, &input.run_id, cancellation.clone())?;
    if let Err(error) = begin_simple_runtime_run(
        state,
        &input.run_id,
        &input.conversation_id,
        "voice.speak",
        "system-tts",
    ) {
        remove_active_run(state, &input.run_id);
        return Err(error);
    }

    let spawn_result = (|| {
        let mut process = state
            .tts_process
            .lock()
            .map_err(|_| "TTS process lock unavailable".to_string())?;
        if process.is_some() {
            return Err("Another speech run is already active".to_string());
        }
        if state.meeting.blocks_tts() {
            return Err(
                "MEETING_POLICY_TTS_BLOCKED: Speech is disabled during a meeting.".to_string(),
            );
        }
        *process = Some(ActiveTts {
            run_id: input.run_id.clone(),
            child: spawn_tts_process(&speech_text, &input.voice)?,
        });
        Ok::<(), String>(())
    })();
    if let Err(error) = spawn_result {
        remove_active_run(state, &input.run_id);
        finish_runtime_run(state, &input.run_id, "failed", Some(&error))?;
        return Err(error);
    }
    state
        .situation
        .set_audio_state(crate::situation::contracts::AudioState::SaaaSpeaking);

    let result: Result<(), String> = async {
        loop {
            if cancellation.is_cancelled() {
                break Err("Speech cancelled".to_string());
            }
            let status = {
                let mut process = state
                    .tts_process
                    .lock()
                    .map_err(|_| "TTS process lock unavailable".to_string())?;
                let active = process
                    .as_mut()
                    .filter(|active| active.run_id == input.run_id)
                    .ok_or_else(|| "Speech process ownership was lost".to_string())?;
                active
                    .child
                    .try_wait()
                    .map_err(|error| format!("Could not inspect TTS process: {error}"))?
            };
            if let Some(status) = status {
                if status.success() {
                    break Ok(());
                }
                break Err(format!("System TTS exited with {status}"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    .await;

    if let Ok(mut process) = state.tts_process.lock() {
        if process
            .as_ref()
            .is_some_and(|active| active.run_id == input.run_id)
        {
            if let Some(mut active) = process.take() {
                if cancellation.is_cancelled() {
                    let _ = active.child.kill();
                }
                let _ = active.child.wait();
            }
            state
                .situation
                .set_audio_state(crate::situation::contracts::AudioState::Silent);
        }
    }
    remove_active_run(state, &input.run_id);
    match result {
        Ok(()) => finish_runtime_run(state, &input.run_id, "completed", None),
        Err(error) if cancellation.is_cancelled() => {
            finish_runtime_run(state, &input.run_id, "cancelled", Some(&error))?;
            Err(error)
        }
        Err(error) => {
            finish_runtime_run(state, &input.run_id, "failed", Some(&error))?;
            Err(error)
        }
    }
}

pub(crate) fn stop_tts(state: &AppState, run_id: String) -> Result<(), String> {
    validate_identifier(&run_id, "run id")?;
    let mut process = state
        .tts_process
        .lock()
        .map_err(|_| "TTS process lock unavailable".to_string())?;
    if process
        .as_ref()
        .is_none_or(|active| active.run_id != run_id)
    {
        return Ok(());
    }
    {
        let active_runs = state
            .active_runs
            .lock()
            .map_err(|_| "Runtime run lock unavailable".to_string())?;
        if let Some(cancellation) = active_runs.get(&run_id) {
            cancellation.cancel();
        }
    }
    if let Some(mut active) = process.take() {
        let _ = active.child.kill();
        let _ = active.child.wait();
    }
    state
        .situation
        .set_audio_state(crate::situation::contracts::AudioState::Silent);
    Ok(())
}

pub(crate) fn spawn_tts_process(text: &str, voice: &str) -> Result<Child, String> {
    let mut command = match env::consts::OS {
        "macos" => {
            let mut command = Command::new("say");
            if voice != "default" {
                command.arg("-v").arg(voice);
            }
            command.arg(text);
            command
        }
        "linux" => {
            let mut command = Command::new("espeak-ng");
            if voice != "default" {
                command.arg("-v").arg(voice);
            }
            command.arg(text);
            command
        }
        "windows" => {
            let escaped = text.replace('\\', "\\\\").replace('\'', "''");
            let mut command = Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('{escaped}')"),
            ]);
            command
        }
        _ => return Err("System TTS is not supported on this platform".to_string()),
    };
    command.stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|error| {
        format!("Could not start local system TTS: {error}. Install the platform speech runtime and retry.")
    })
}
