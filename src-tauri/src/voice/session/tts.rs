use crate::{validate_identifier, AppState};

#[derive(Clone)]
pub(crate) enum TtsRoute {
    Harness(String),
    Cloud(crate::CloudTtsProviderSettings),
    System(crate::SystemTtsProviderSettings),
}

pub(crate) fn selected_tts_route(state: &AppState) -> Result<(TtsRoute, String, u64), String> {
    let (providers, route) = state.sqlite_readers.read(|connection| {
        Ok((
            crate::persistence::load_model_providers(connection)?,
            crate::persistence::load_routing_settings(connection)?.voice_speak,
        ))
    })?;
    if route.source == "harness" {
        return Ok((
            TtsRoute::Harness(providers.harness.address),
            "provider-harness-tts".to_string(),
            route.timeout_ms,
        ));
    }
    let provider_id = route
        .provider_id
        .as_deref()
        .ok_or_else(|| "TTS provider is not selected".to_string())?;
    let selected = providers
        .providers
        .into_iter()
        .find_map(|provider| match provider {
            crate::ModelProviderSettings::CloudTts(provider)
                if provider.id == provider_id && provider.enabled =>
            {
                Some(TtsRoute::Cloud(provider))
            }
            crate::ModelProviderSettings::SystemTts(provider)
                if provider.id == provider_id && provider.enabled =>
            {
                Some(TtsRoute::System(provider))
            }
            _ => None,
        })
        .ok_or_else(|| "The selected TTS provider is unavailable".to_string())?;
    Ok((selected, provider_id.to_string(), route.timeout_ms))
}

pub(crate) fn stop_tts(state: &AppState, run_id: String) -> Result<(), String> {
    validate_identifier(&run_id, "run id")?;
    if let Ok(active_runs) = state.active_runs.lock() {
        if let Some(cancellation) = active_runs.get(&run_id) {
            cancellation.cancel();
        }
    }
    state.streaming_tts.cancel(&run_id);
    Ok(())
}
