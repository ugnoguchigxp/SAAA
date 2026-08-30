use std::collections::hash_map::Entry;
use std::sync::Arc;

use zeroize::Zeroizing;

use crate::redact::redact_runtime_text;
use crate::{
    begin_simple_runtime_run, finish_runtime_run, register_active_run, remove_active_run,
    validate_identifier, AppState, RunCancellation, TranscribeAudioChunkInput,
    TranscribeAudioInput, VoiceEvent,
};

enum AsrRoute {
    Harness(String),
    Cloud(crate::CloudAsrProviderSettings),
}

struct SelectedAsr {
    route: AsrRoute,
    provider_id: String,
    timeout_ms: u64,
    allowed_languages: Vec<String>,
    vad_sensitivity: String,
}

pub(crate) async fn transcribe_audio(
    state: &AppState,
    input: TranscribeAudioInput,
    on_event: tauri::ipc::Channel<VoiceEvent>,
) -> Result<String, String> {
    validate_identifier(&input.run_id, "run id")?;
    validate_identifier(&input.conversation_id, "conversation id")?;
    if input.sample_rate != 16_000 {
        return Err("Recorded audio must use the canonical 16 kHz sample rate".to_string());
    }
    let samples = Zeroizing::new(
        state
            .audio_uploads
            .consume(&input.audio_upload_id, "chat-asr")?,
    );
    if samples.is_empty() || samples.iter().any(|sample| !sample.is_finite()) {
        return Err("Recorded audio is empty or invalid".to_string());
    }
    if samples.len() > input.sample_rate as usize * 120 {
        return Err("Recording exceeds the two minute limit".to_string());
    }
    let (verification, selected) = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        (
            state
                .voice_profile
                .verify_if_enabled(&connection, &samples, input.sample_rate),
            select_asr(&connection)?,
        )
    };
    validate_asr_audio_quality(&samples, input.sample_rate, &selected.vad_sensitivity)?;
    if let Err(error) = verification {
        let _ = on_event.send(VoiceEvent::Failed {
            run_id: input.run_id.clone(),
            message: error.clone(),
            recovery: "Use the enrolled microphone and speak clearly, or disable the target-speaker filter in Settings.".to_string(),
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
        &selected.provider_id,
    ) {
        remove_active_run(state, &input.run_id);
        return Err(error);
    }
    let _ = on_event.send(VoiceEvent::Transcribing {
        run_id: input.run_id.clone(),
    });
    state
        .situation
        .set_microphone_state(crate::situation::contracts::MicrophoneState::SaaaTranscribing);

    let result = transcribe_selected(
        state,
        &samples,
        input.sample_rate,
        selected,
        cancellation.clone(),
    )
    .await
    .map(|(text, _)| text);
    remove_active_run(state, &input.run_id);
    state
        .situation
        .set_microphone_state(crate::situation::contracts::MicrophoneState::Inactive);
    finish_transcription(state, input.run_id, result, cancellation, on_event)
}

pub(crate) async fn transcribe_audio_chunk(
    state: &AppState,
    input: TranscribeAudioChunkInput,
    on_event: tauri::ipc::Channel<VoiceEvent>,
) -> Result<String, String> {
    validate_identifier(&input.run_id, "run id")?;
    if input.sample_rate != 16_000 {
        return Err("Streaming ASR chunks must use the canonical 16 kHz sample rate".to_string());
    }
    let samples = Zeroizing::new(
        state
            .audio_uploads
            .consume(&input.audio_upload_id, "chat-asr-chunk")?,
    );
    validate_streaming_chunk(&samples, input.sample_rate)?;
    let selected = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        state
            .voice_profile
            .verify_if_enabled(&connection, &samples, input.sample_rate)?;
        select_asr(&connection)?
    };

    let cancellation = Arc::new(RunCancellation::default());
    register_streaming_run(state, &input.run_id, cancellation.clone())?;
    let _ = on_event.send(VoiceEvent::Transcribing {
        run_id: input.run_id.clone(),
    });
    let result = transcribe_selected(
        state,
        &samples,
        input.sample_rate,
        selected,
        cancellation.clone(),
    )
    .await;
    remove_active_run(state, &input.run_id);
    match result {
        Ok((text, _)) => Ok(text),
        Err(_) if cancellation.is_cancelled() => {
            Err("Streaming transcription cancelled".to_string())
        }
        Err(error) => Err(redact_runtime_text(&error)),
    }
}

fn register_streaming_run(
    state: &AppState,
    run_id: &str,
    cancellation: Arc<RunCancellation>,
) -> Result<(), String> {
    let mut active = state
        .active_runs
        .lock()
        .map_err(|_| "Run registry lock unavailable".to_string())?;
    match active.entry(run_id.to_string()) {
        Entry::Vacant(entry) => {
            entry.insert(cancellation);
        }
        Entry::Occupied(_) => return Err("A run with this id is already active".to_string()),
    }
    Ok(())
}

fn validate_streaming_chunk(samples: &[f32], sample_rate: u32) -> Result<(), String> {
    let minimum_samples = sample_rate as usize / 2;
    let maximum_samples = sample_rate as usize * 30;
    if samples.len() < minimum_samples
        || samples.len() > maximum_samples
        || samples.iter().any(|sample| !sample.is_finite())
    {
        return Err(
            "Streaming ASR chunks must contain between 0.5 and 30 seconds of valid audio"
                .to_string(),
        );
    }
    Ok(())
}

fn select_asr(connection: &rusqlite::Connection) -> Result<SelectedAsr, String> {
    let voice = crate::persistence::load_voice_settings(connection)?;
    let providers = crate::persistence::load_model_providers(connection)?;
    let settings = crate::persistence::load_routing_settings(connection)?.voice_transcribe;
    let route = if settings.source == "harness" {
        AsrRoute::Harness(providers.harness.address)
    } else {
        let provider_id = settings
            .provider_id
            .as_deref()
            .ok_or_else(|| "ASR provider is not selected".to_string())?;
        let provider = providers
            .providers
            .into_iter()
            .find_map(|provider| match provider {
                crate::ModelProviderSettings::CloudAsr(provider)
                    if provider.id == provider_id && provider.enabled =>
                {
                    Some(provider)
                }
                _ => None,
            })
            .ok_or_else(|| "The selected ASR provider is unavailable".to_string())?;
        AsrRoute::Cloud(provider)
    };
    let provider_id = match &route {
        AsrRoute::Harness(_) => "provider-harness-asr".to_string(),
        AsrRoute::Cloud(provider) => provider.id.clone(),
    };
    Ok(SelectedAsr {
        route,
        provider_id,
        timeout_ms: settings.timeout_ms,
        allowed_languages: voice.allowed_languages,
        vad_sensitivity: voice.vad_sensitivity,
    })
}

pub(crate) async fn transcribe_selected_audio(
    state: &AppState,
    samples: &[f32],
    sample_rate: u32,
    cancellation: Arc<RunCancellation>,
) -> Result<(String, Option<String>), String> {
    let selected = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        select_asr(&connection)?
    };
    validate_asr_audio_quality(samples, sample_rate, &selected.vad_sensitivity)?;
    transcribe_selected(state, samples, sample_rate, selected, cancellation).await
}

pub(crate) async fn probe_selected_asr(state: &AppState) -> Result<(), String> {
    let selected = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        select_asr(&connection)?
    };
    match selected.route {
        AsrRoute::Cloud(provider) => crate::voice::cloud_asr::probe(&provider).await.map(|_| ()),
        AsrRoute::Harness(address) => {
            match crate::providers::service_harness::resolve_service(&address, "asr").await {
                Ok(service) => crate::voice::cloud_asr::probe(&harness_asr_provider(service))
                    .await
                    .map(|_| ()),
                Err(error) => {
                    match crate::providers::service_harness::legacy_dynamic_lan_host(&address)? {
                        Some(host) => state
                            .network_asr
                            .resolve(&host, Arc::new(RunCancellation::default()))
                            .await
                            .map(|_| ()),
                        None => Err(error),
                    }
                }
            }
        }
    }
}

fn harness_asr_provider(
    service: crate::providers::service_harness::ServiceDescriptor,
) -> crate::CloudAsrProviderSettings {
    crate::CloudAsrProviderSettings {
        id: "provider-harness-asr".to_string(),
        enabled: true,
        label: "Provider Harness ASR".to_string(),
        location: "local".to_string(),
        endpoint: service.base_url,
        model: service.model,
        language: "auto".to_string(),
        authentication: "none".to_string(),
    }
}

async fn transcribe_selected(
    state: &AppState,
    samples: &[f32],
    sample_rate: u32,
    selected: SelectedAsr,
    cancellation: Arc<RunCancellation>,
) -> Result<(String, Option<String>), String> {
    let timeout_ms = selected.timeout_ms;
    let result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
        match selected.route {
            AsrRoute::Cloud(provider) => {
                crate::voice::cloud_asr::transcribe(
                    &provider,
                    samples,
                    sample_rate,
                    timeout_ms,
                    cancellation,
                )
                .await
            }
            AsrRoute::Harness(address) => {
                match crate::providers::service_harness::resolve_service_cancellable(
                    &address,
                    "asr",
                    &cancellation,
                )
                .await
                {
                    Ok(service) => {
                        let provider = harness_asr_provider(service);
                        crate::voice::cloud_asr::transcribe(
                            &provider,
                            samples,
                            sample_rate,
                            timeout_ms,
                            cancellation,
                        )
                        .await
                    }
                    Err(error) => {
                        match crate::providers::service_harness::legacy_dynamic_lan_host(&address)?
                        {
                            Some(host) => {
                                crate::voice::network_asr::transcribe(
                                    state,
                                    &host,
                                    samples,
                                    sample_rate,
                                    crate::voice::network_asr::MODEL_ID,
                                    cancellation,
                                )
                                .await
                            }
                            None => Err(error),
                        }
                    }
                }
            }
        }
    })
    .await
    .map_err(|_| "ASR request reached its configured timeout".to_string())??;
    crate::voice::language::enforce_allowed_language(
        result.1.as_deref(),
        &selected.allowed_languages,
    )?;
    Ok(result)
}

fn finish_transcription(
    state: &AppState,
    run_id: String,
    result: Result<String, String>,
    cancellation: Arc<RunCancellation>,
    on_event: tauri::ipc::Channel<VoiceEvent>,
) -> Result<String, String> {
    match result {
        Ok(transcript) => {
            finish_runtime_run(state, &run_id, "completed", None)?;
            let _ = on_event.send(VoiceEvent::TranscriptFinal {
                run_id,
                text: transcript.clone(),
            });
            Ok(transcript)
        }
        Err(_) if cancellation.is_cancelled() => {
            finish_runtime_run(state, &run_id, "cancelled", Some("Cancelled by user"))?;
            let _ = on_event.send(VoiceEvent::Cancelled { run_id });
            Err("Transcription cancelled".to_string())
        }
        Err(error) => {
            let error = redact_runtime_text(&error);
            finish_runtime_run(state, &run_id, "failed", Some(&error))?;
            let recovery = if error.starts_with("ASR_LANGUAGE_NOT_ALLOWED") {
                "Register the language in Voice settings if you want SAAA to transcribe it."
            } else if error.starts_with("ASR_LANGUAGE_UNKNOWN") {
                "Speak clearly in one of the languages registered in Voice settings."
            } else if error.starts_with("ASR_NO_SPEECH") {
                "Speak closer to the microphone and retry."
            } else {
                "Check the selected ASR service and credential, then retry."
            };
            let _ = on_event.send(VoiceEvent::Failed {
                run_id,
                message: error.clone(),
                recovery: recovery.to_string(),
            });
            Err(error)
        }
    }
}

fn validate_asr_audio_quality(
    samples: &[f32],
    sample_rate: u32,
    vad_sensitivity: &str,
) -> Result<(), String> {
    const MIN_SPEECH_MS: usize = 240;
    let minimum_speech_rms = match vad_sensitivity {
        "high" => 0.006,
        "low" => 0.012,
        _ => 0.008,
    };
    if samples.len() < sample_rate as usize / 2 {
        return Err("ASR_NO_SPEECH: Recorded audio is too short".to_string());
    }
    let frame_size = (sample_rate as usize / 50).max(1);
    let required_frames = (MIN_SPEECH_MS * 50).div_ceil(1_000);
    let voiced_frames = samples
        .chunks(frame_size)
        .filter(|frame| {
            if frame.is_empty() {
                return false;
            }
            let mean = frame.iter().sum::<f32>() / frame.len() as f32;
            let centered_rms = (frame
                .iter()
                .map(|sample| (sample - mean).powi(2))
                .sum::<f32>()
                / frame.len() as f32)
                .sqrt();
            centered_rms >= minimum_speech_rms
        })
        .count();
    if voiced_frames < required_frames {
        return Err("ASR_NO_SPEECH: Recorded audio contains too little speech".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asr_audio_quality_accepts_quiet_speech_and_rejects_silence() {
        let mut quiet_speech = vec![0.0; 16_000];
        for (index, sample) in quiet_speech[..4_000].iter_mut().enumerate() {
            *sample = if index % 2 == 0 { 0.009 } else { -0.009 };
        }
        assert!(validate_asr_audio_quality(&quiet_speech, 16_000, "medium").is_ok());
        assert!(validate_asr_audio_quality(&[0.0; 16_000], 16_000, "medium").is_err());
        assert!(validate_asr_audio_quality(&[0.02; 16_000], 16_000, "medium").is_err());
        assert!(validate_asr_audio_quality(&quiet_speech, 16_000, "low").is_err());
    }

    #[test]
    fn streaming_asr_chunk_bounds_are_finite_and_short_lived() {
        assert!(validate_streaming_chunk(&[0.0; 8_000], 16_000).is_ok());
        assert!(validate_streaming_chunk(&[0.0; 7_999], 16_000).is_err());
        assert!(validate_streaming_chunk(&[0.0; 480_001], 16_000).is_err());
        let mut invalid = vec![0.0; 8_000];
        invalid[0] = f32::NAN;
        assert!(validate_streaming_chunk(&invalid, 16_000).is_err());
    }
}
