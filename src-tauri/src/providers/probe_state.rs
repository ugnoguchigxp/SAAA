use crate::{now_iso, AppState, ModelProviderSettings, ProviderProbeStatus};

pub(super) fn record_if_current(state: &AppState, provider: &ModelProviderSettings, ok: bool) {
    let Ok(tested_configuration) = serde_json::to_value(provider) else {
        return;
    };
    let matches_saved_configuration = state
        .connection
        .lock()
        .ok()
        .and_then(|connection| crate::persistence::load_model_providers(&connection).ok())
        .is_some_and(|settings| {
            settings.providers.iter().any(|saved| {
                saved.id() == provider.id()
                    && serde_json::to_value(saved).ok().as_ref() == Some(&tested_configuration)
            })
        });
    if !matches_saved_configuration {
        return;
    }
    state
        .provider_probes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            provider.id().to_string(),
            ProviderProbeStatus {
                ok,
                checked_at: now_iso(),
            },
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsaved_provider_edits_do_not_change_effective_readiness() {
        let connection = rusqlite::Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        let state = crate::test_support::app_state(connection);
        let saved = {
            let connection = state.connection.lock().expect("database lock");
            crate::persistence::load_model_providers(&connection)
                .expect("providers load")
                .providers
                .into_iter()
                .next()
                .expect("provider exists")
        };
        record_if_current(&state, &saved, true);
        assert!(state
            .provider_probes
            .lock()
            .expect("probe lock")
            .contains_key(saved.id()));

        state.provider_probes.lock().expect("probe lock").clear();
        let mut draft = saved;
        if let ModelProviderSettings::DynamicLan(provider) = &mut draft {
            provider.host = "10.0.0.42".to_string();
        }
        record_if_current(&state, &draft, false);
        assert!(state.provider_probes.lock().expect("probe lock").is_empty());
    }
}
