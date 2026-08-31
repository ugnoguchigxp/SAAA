use std::sync::Arc;

use crate::{AppState, RunCancellation};

pub(crate) enum AsrRoute {
    Harness(String),
    Cloud(crate::CloudAsrProviderSettings),
}

pub(crate) struct SelectedAsr {
    pub(crate) route: AsrRoute,
    pub(crate) timeout_ms: u64,
    pub(crate) allowed_languages: Vec<String>,
    pub(crate) vad_sensitivity: String,
}

pub(crate) fn select_asr(connection: &rusqlite::Connection) -> Result<SelectedAsr, String> {
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
    Ok(SelectedAsr {
        route,
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
    let selected = state.sqlite_readers.read(select_asr)?;
    validate_asr_audio_quality(samples, sample_rate, &selected.vad_sensitivity)?;
    transcribe_selected(state, samples, sample_rate, selected, cancellation).await
}

pub(crate) async fn probe_selected_asr(state: &AppState) -> Result<(), String> {
    let selected = state.sqlite_readers.read(select_asr)?;
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

pub(crate) fn harness_asr_provider(
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

fn validate_asr_audio_quality(
    samples: &[f32],
    sample_rate: u32,
    vad_sensitivity: &str,
) -> Result<(), String> {
    const MIN_SPEECH_MS: usize = 240;
    let minimum_speech_rms = vad_rms_threshold(vad_sensitivity);
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

pub(crate) fn vad_rms_threshold(vad_sensitivity: &str) -> f32 {
    match vad_sensitivity {
        "high" => 0.006,
        "low" => 0.012,
        _ => 0.008,
    }
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
}
