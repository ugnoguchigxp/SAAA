use crate::{AppState, LocalArtifactResult};
use serde_json::json;
use std::env;
use std::fs;

mod database;
#[cfg(test)]
use database::build_provider_diagnostics;

pub(crate) fn export_diagnostics(state: &AppState) -> Result<LocalArtifactResult, String> {
    let database = database::load(&state.sqlite_readers)?;
    let created_at = crate::now_iso();
    let payload = json!({
        "format": "saaa-diagnostics-v2",
        "createdAt": created_at,
        "redacted": true,
        "application": { "version": env!("CARGO_PKG_VERSION"), "platform": env::consts::OS, "arch": env::consts::ARCH },
        "database": database.database,
        "situation": database.situation,
        "recentRuns": database.recent_runs,
        "providerSessions": database.provider_sessions,
        "streamingPerformance": crate::runtime::event_hub::performance::snapshot(),
        "auditTrail": database.audit_trail
    });
    let directory = state.data_directory.join("diagnostics");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the diagnostics directory: {error}"))?;
    let path = directory.join(format!("saaa-diagnostics-{created_at}.json"));
    let contents = serde_json::to_vec_pretty(&payload)
        .map_err(|error| format!("Could not encode diagnostics: {error}"))?;
    fs::write(&path, contents).map_err(|error| format!("Could not write diagnostics: {error}"))?;
    Ok(LocalArtifactResult {
        path: path.to_string_lossy().into_owned(),
        created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn provider_diagnostics_exclude_allocation_and_request_identifiers() {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO provider_sessions(
                   id,provider_id,runtime_run_id,provider_kind,route_id,allocation_id,
                   selected_runtime_id,fallback_used,selection_reason,request_id,output_started,
                   release_status,status,started_at,updated_at
                 ) VALUES(
                   'session_diag','larm-primary','run_diag','larm','llm-default','alloc_secret',
                   'runtime_safe',0,'primary','req_secret',1,'released','completed','1','2'
                 )",
                [],
            )
            .expect("diagnostic fixture inserts");
        let diagnostics = build_provider_diagnostics(&connection).expect("diagnostics build");
        let encoded = diagnostics.to_string();
        assert!(encoded.contains("runtime_safe"));
        assert!(encoded.contains("llm-default"));
        for forbidden in ["alloc_secret", "req_secret", "allocationId", "requestId"] {
            assert!(
                !encoded.contains(forbidden),
                "diagnostics exposed {forbidden}"
            );
        }
    }
}
