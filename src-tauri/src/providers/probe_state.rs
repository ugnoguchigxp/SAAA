use crate::{now_iso, AppState, ModelProviderSettings, ProviderProbeStatus};

pub(super) struct ProbeConfigurationSnapshot {
    fingerprint: String,
    prior_session_rowid: i64,
}

pub(super) fn capture_if_current(
    state: &AppState,
    provider: &ModelProviderSettings,
) -> Option<ProbeConfigurationSnapshot> {
    let tested_configuration = serde_json::to_value(provider).ok()?;
    let connection = state.connection.lock().ok()?;
    let settings = crate::persistence::load_model_providers(&connection).ok()?;
    let matches_saved = settings.providers.iter().any(|saved| {
        saved.id() == provider.id()
            && serde_json::to_value(saved).ok().as_ref() == Some(&tested_configuration)
    });
    if !matches_saved {
        return None;
    }
    let fingerprint =
        crate::persistence::effective_route::load_conversation_configuration_fingerprint(
            &connection,
        )
        .ok()?;
    let prior_session_rowid = connection
        .query_row(
            "SELECT COALESCE(MAX(rowid), 0) FROM provider_sessions
             WHERE configuration_fingerprint=?1",
            [&fingerprint],
            |row| row.get(0),
        )
        .ok()?;
    Some(ProbeConfigurationSnapshot {
        fingerprint,
        prior_session_rowid,
    })
}

pub(super) fn record_if_current(
    state: &AppState,
    provider: &ModelProviderSettings,
    captured: Option<ProbeConfigurationSnapshot>,
    ok: bool,
) {
    let Some(captured) = captured else {
        return;
    };
    let Ok(tested_configuration) = serde_json::to_value(provider) else {
        return;
    };
    let matches_saved_configuration = state.connection.lock().ok().and_then(|connection| {
        let settings = crate::persistence::load_model_providers(&connection).ok()?;
        let fingerprint =
            crate::persistence::effective_route::load_conversation_configuration_fingerprint(
                &connection,
            )
            .ok()?;
        Some(
            fingerprint == captured.fingerprint
                && settings.providers.iter().any(|saved| {
                    saved.id() == provider.id()
                        && serde_json::to_value(saved).ok().as_ref() == Some(&tested_configuration)
                }),
        )
    });
    if matches_saved_configuration != Some(true) {
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
                configuration_fingerprint: captured.fingerprint,
                prior_session_rowid: captured.prior_session_rowid,
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
        let captured = capture_if_current(&state, &saved);
        record_if_current(&state, &saved, captured, true);
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
        let captured = capture_if_current(&state, &draft);
        record_if_current(&state, &draft, captured, false);
        assert!(state.provider_probes.lock().expect("probe lock").is_empty());

        let saved = {
            let connection = state.connection.lock().expect("database lock");
            crate::persistence::load_model_providers(&connection)
                .expect("providers load")
                .providers
                .into_iter()
                .next()
                .expect("provider exists")
        };
        let captured = capture_if_current(&state, &saved);
        state
            .connection
            .lock()
            .expect("database lock")
            .execute(
                "UPDATE settings_documents
                 SET value_json=json_set(value_json, '$.conversationRespond.timeoutMs', 31000)
                 WHERE namespace='routing.tasks' AND key='default'",
                [],
            )
            .expect("routing changes while probe runs");
        record_if_current(&state, &saved, captured, true);
        assert!(state.provider_probes.lock().expect("probe lock").is_empty());
    }
}
