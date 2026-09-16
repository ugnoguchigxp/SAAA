#[path = "tts_policy.rs"]
mod policy;
use crate::{validate_identifier, AppState};

#[derive(Clone)]
pub(crate) enum TtsRoute {
    Larm(String, crate::HarnessSettings),
    Fallback(Vec<TtsRoute>, u64),
    Harness(String, Option<String>),
    Cloud(crate::CloudTtsProviderSettings),
    System(crate::SystemTtsProviderSettings),
}

pub(crate) fn selected_tts_route(state: &AppState) -> Result<(TtsRoute, String, u64), String> {
    let (providers, route, security) = state.sqlite_readers.read(|connection| {
        Ok((
            crate::persistence::load_model_providers(connection)?,
            crate::persistence::load_routing_settings(connection)?.voice_speak,
            crate::persistence::load_security_settings(connection)?,
        ))
    })?;
    let wrap = |primary| policy::wrap(primary, &providers, &route, &security);
    if route.source == "harness" {
        return Ok((
            wrap(TtsRoute::Harness(
                providers.harness.address.clone(),
                providers.harness.tts_voice.clone(),
            )),
            "provider-harness-tts".to_string(),
            route.timeout_ms,
        ));
    }
    let provider_id = route
        .provider_id
        .as_deref()
        .ok_or_else(|| "TTS provider is not selected".to_string())?;
    let provider = providers
        .providers
        .iter()
        .find(|p| p.id() == provider_id && p.enabled())
        .ok_or("The selected TTS provider is unavailable")?;
    let selected = match provider {
        crate::ModelProviderSettings::CloudTts(p) => TtsRoute::Cloud(p.clone()),
        crate::ModelProviderSettings::SystemTts(p) => TtsRoute::System(p.clone()),
        _ => return Err("The selected provider does not support TTS".into()),
    };
    Ok((wrap(selected), provider_id.to_string(), route.timeout_ms))
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Arc;

    fn state() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    #[test]
    fn default_settings_select_the_system_tts_provider() {
        let (route, provider_id, timeout_ms) = selected_tts_route(&state()).expect("tts route");
        assert!(matches!(route, TtsRoute::System(_)));
        assert_eq!(provider_id, "system-tts");
        assert_eq!(timeout_ms, 30_000);
    }

    #[test]
    fn stop_tts_validates_the_run_id_and_cancels_an_active_run() {
        let state = state();
        assert!(stop_tts(&state, "bad run".into()).is_err());
        stop_tts(&state, "run_idle".into()).expect("missing speech session is ignored");
        let cancellation = Arc::new(crate::RunCancellation::default());
        state
            .active_runs
            .lock()
            .expect("active runs")
            .insert("run_speech".into(), cancellation.clone());
        stop_tts(&state, "run_speech".into()).expect("active run cancels");
        assert!(cancellation.is_cancelled());
    }
}
