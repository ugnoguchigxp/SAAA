#[derive(Clone)]
pub(crate) enum AsrRoute {
    Harness(String),
    Larm(String, crate::HarnessSettings),
    Cloud(crate::CloudAsrProviderSettings),
}

#[derive(Clone)]
pub(crate) struct SelectedAsr {
    pub(crate) route: AsrRoute,
    pub(crate) timeout_ms: u64,
    pub(crate) attempt_timeout_ms: u64,
    pub(crate) fallbacks: Vec<AsrRoute>,
    pub(crate) allowed_languages: Vec<String>,
    pub(crate) vad_sensitivity: String,
}

pub(crate) fn select_streaming_asr(
    connection: &rusqlite::Connection,
) -> Result<SelectedAsr, String> {
    select_route(connection)
}

fn select_route(connection: &rusqlite::Connection) -> Result<SelectedAsr, String> {
    let voice = crate::persistence::load_voice_settings(connection)?;
    let providers = crate::persistence::load_model_providers(connection)?;
    let settings = crate::persistence::load_routing_settings(connection)?.voice_transcribe;
    let route = if settings.source == "harness" {
        AsrRoute::Harness(providers.harness.address.clone())
    } else {
        let provider_id = settings
            .provider_id
            .as_deref()
            .ok_or_else(|| "ASR provider is not selected".to_string())?;
        let provider = providers
            .providers
            .iter()
            .cloned()
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
    let security = crate::persistence::load_security_settings(connection)?;
    let primary_local = settings.source == "harness"
        || matches!(&route, AsrRoute::Cloud(p) if p.location == "local");
    let fallbacks = settings
        .fallback_provider_ids
        .iter()
        .filter_map(|id| {
            providers.providers.iter().find_map(|p| match p {
                crate::ModelProviderSettings::CloudAsr(p)
                    if &p.id == id
                        && p.enabled
                        && !(security.local_only_when_selected
                            && primary_local
                            && p.location == "cloud") =>
                {
                    Some(AsrRoute::Cloud(p.clone()))
                }
                _ => None,
            })
        })
        .collect();
    Ok(SelectedAsr {
        route,
        timeout_ms: settings.timeout_ms,
        attempt_timeout_ms: settings
            .attempt_timeout_ms
            .unwrap_or(settings.timeout_ms / (1 + settings.fallback_provider_ids.len()) as u64),
        fallbacks,
        allowed_languages: voice.allowed_languages,
        vad_sensitivity: voice.vad_sensitivity,
    })
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
    fn default_settings_select_the_harness_asr_route() {
        let connection = rusqlite::Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let streaming = select_streaming_asr(&connection).expect("streaming asr route");
        assert!(matches!(streaming.route, AsrRoute::Harness(_)));
        assert_eq!(vad_rms_threshold("high"), 0.006);
        assert_eq!(vad_rms_threshold("low"), 0.012);
        assert_eq!(vad_rms_threshold("medium"), 0.008);
        let provider = harness_asr_provider(crate::providers::service_harness::ServiceDescriptor {
            capability: "asr".into(),
            protocol: "openai.audio-transcriptions.v1".into(),
            base_url: "http://127.0.0.1:9/v1".into(),
            model: "harness-asr".into(),
            language: None,
            voice: None,
            health_url: "http://127.0.0.1:9/health".into(),
        });
        assert_eq!(provider.id, "provider-harness-asr");
        assert_eq!(provider.model, "harness-asr");
    }
}
