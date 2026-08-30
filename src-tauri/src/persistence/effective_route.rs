use std::collections::HashMap;

use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};

use super::{load_model_providers, load_routing_settings};
use crate::{
    database_error, ConversationRouteSettings, EffectiveRouteSnapshot, ModelProviderSettings,
    ModelProvidersSettings, ProviderProbeStatus,
};

pub(crate) fn conversation_configuration_fingerprint(
    providers: &ModelProvidersSettings,
    route: &ConversationRouteSettings,
) -> Result<String, String> {
    let encoded = serde_json::to_vec(&(providers, route))
        .map_err(|error| format!("Could not fingerprint conversation configuration: {error}"))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

pub(crate) fn load_conversation_configuration_fingerprint(
    connection: &rusqlite::Connection,
) -> Result<String, String> {
    let providers = load_model_providers(connection)?;
    let route = load_routing_settings(connection)?.conversation_respond;
    conversation_configuration_fingerprint(&providers, &route)
}

pub(crate) fn effective_route_snapshot(
    connection: &rusqlite::Connection,
    provider_probes: &HashMap<String, ProviderProbeStatus>,
) -> Result<EffectiveRouteSnapshot, String> {
    let providers = load_model_providers(connection)?;
    let route = load_routing_settings(connection)?.conversation_respond;
    let configuration_fingerprint = conversation_configuration_fingerprint(&providers, &route)?;
    let configured = providers
        .providers
        .iter()
        .find(|provider| provider.id() == route.primary_provider_id && provider.enabled());
    let unchecked = || EffectiveRouteSnapshot {
        provider_id: configured.map(|provider| provider.id().to_string()),
        label: configured
            .map(|provider| provider.label().to_string())
            .unwrap_or_else(|| "モデル未選択".to_string()),
        location: configured.map(|provider| provider.location().to_string()),
        state: "unchecked".to_string(),
        fallback_used: false,
        reason_code: "no-completed-provider-session".to_string(),
        updated_at: None,
    };
    let primary_probe = configured.and_then(|provider| {
        provider_probes
            .get(provider.id())
            .map(|probe| (provider, probe))
    });
    let current_primary_probe = primary_probe
        .filter(|(_, probe)| probe.configuration_fingerprint == configuration_fingerprint);
    let latest = connection
        .query_row(
            "SELECT ps.rowid, ps.provider_id, ps.status, COALESCE(ps.fallback_used, 0),
                    ps.selection_reason, ps.failure_kind, ps.updated_at
             FROM provider_sessions ps
             JOIN runtime_runs rr ON rr.id = ps.runtime_run_id
             WHERE rr.route_kind = 'conversation.respond'
               AND ps.configuration_fingerprint = ?1
             ORDER BY ps.rowid DESC
             LIMIT 1",
            [&configuration_fingerprint],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some((
        session_rowid,
        provider_id,
        status,
        provider_fallback,
        selection_reason,
        failure_kind,
        updated_at,
    )) = latest
    else {
        return Ok(current_primary_probe
            .map(|(provider, probe)| probe_snapshot(provider, probe))
            .unwrap_or_else(unchecked));
    };
    if let Some((provider, probe)) = current_primary_probe {
        if probe.prior_session_rowid >= session_rowid {
            return Ok(probe_snapshot(provider, probe));
        }
    }
    let Some(provider) = providers
        .providers
        .iter()
        .find(|provider| provider.id() == provider_id && provider.enabled())
    else {
        return Ok(unchecked());
    };
    let state = match status.as_str() {
        "completed" => "ready",
        "running" => "active",
        "failed" => "failed",
        _ => "unchecked",
    };
    let route_fallback = provider_id != route.primary_provider_id;
    let reason_code = if let Some(failure_kind) = failure_kind {
        format!("provider-{failure_kind}")
    } else if route_fallback {
        "fallback-route".to_string()
    } else if selection_reason.as_deref() == Some("other") {
        "provider-selected-other".to_string()
    } else if state == "active" {
        "turn-active".to_string()
    } else if state == "ready" {
        "last-turn-completed".to_string()
    } else {
        "last-turn-not-ready".to_string()
    };
    Ok(EffectiveRouteSnapshot {
        provider_id: Some(provider_id),
        label: provider.label().to_string(),
        location: Some(provider.location().to_string()),
        state: state.to_string(),
        fallback_used: provider_fallback || route_fallback,
        reason_code,
        updated_at: Some(updated_at),
    })
}

fn probe_snapshot(
    provider: &ModelProviderSettings,
    probe: &ProviderProbeStatus,
) -> EffectiveRouteSnapshot {
    EffectiveRouteSnapshot {
        provider_id: Some(provider.id().to_string()),
        label: provider.label().to_string(),
        location: Some(provider.location().to_string()),
        state: if probe.ok { "ready" } else { "failed" }.to_string(),
        fallback_used: false,
        reason_code: if probe.ok {
            "provider-probe-succeeded"
        } else {
            "provider-probe-failed"
        }
        .to_string(),
        updated_at: Some(probe.checked_at.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_backend_probe_controls_readiness_without_endpoint_guessing() {
        let mut connection = rusqlite::Connection::open_in_memory().expect("database opens");
        crate::persistence::schema::initialize_database(&connection).expect("database initializes");
        let mut settings = crate::test_support::default_settings_input();
        let providers = settings
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings exist");
        providers.value_json["providers"][0]["enabled"] = serde_json::json!(true);
        crate::persistence::save_settings_documents_to_connection(&mut connection, &settings)
            .expect("settings save");
        let probes = HashMap::from([(
            crate::DYNAMIC_LAN_PROVIDER_ID.to_string(),
            ProviderProbeStatus {
                ok: true,
                checked_at: crate::now_iso(),
                configuration_fingerprint: load_conversation_configuration_fingerprint(&connection)
                    .expect("configuration fingerprints"),
                prior_session_rowid: 0,
            },
        )]);
        let snapshot = effective_route_snapshot(&connection, &probes).expect("snapshot builds");
        assert_eq!(snapshot.state, "ready");
        assert_eq!(snapshot.reason_code, "provider-probe-succeeded");
        assert_eq!(
            snapshot.provider_id.as_deref(),
            Some(crate::DYNAMIC_LAN_PROVIDER_ID)
        );
    }

    #[test]
    fn session_without_the_current_configuration_fingerprint_is_not_reused() {
        let mut connection = rusqlite::Connection::open_in_memory().expect("database opens");
        crate::persistence::schema::initialize_database(&connection).expect("database initializes");
        let mut settings = crate::test_support::default_settings_input();
        let providers = settings
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings exist");
        providers.value_json["providers"][0]["enabled"] = serde_json::json!(true);
        crate::persistence::save_settings_documents_to_connection(&mut connection, &settings)
            .expect("settings save");
        connection
            .execute_batch(
                "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
                 VALUES('route-test',NULL,'conversation','1','1');
                 INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at)
                 VALUES('route-run','route-test','conversation.respond','completed','1');",
            )
            .expect("run fixture inserts");
        connection
            .execute(
                "INSERT INTO provider_sessions(
                   id,provider_id,runtime_run_id,provider_kind,fallback_used,output_started,
                   release_status,status,started_at,updated_at
                 ) VALUES('route-session',?1,'route-run','openai-compatible',0,1,
                   'not-applicable','completed','1','1')",
                rusqlite::params![crate::DYNAMIC_LAN_PROVIDER_ID],
            )
            .expect("provider session fixture inserts");

        let snapshot =
            effective_route_snapshot(&connection, &HashMap::new()).expect("snapshot builds");
        assert_eq!(snapshot.state, "unchecked");
        assert_eq!(snapshot.reason_code, "no-completed-provider-session");
    }
}
