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
        "auditTrail": database.audit_trail,
        "personalState": state.sqlite_writer.read_serialized(crate::memory::personal_state::commands::summary)?,
        "selfDiagnosis": state.diagnosis.snapshot(),
        "contextMetrics": {
            "usage": state.sqlite_readers.read(crate::runtime::context::usage::summary)?
                .into_iter()
                .map(|row| json!({
                    "providerId": row.provider_id,
                    "generations": row.generations,
                    "inputTokens": row.input_tokens,
                    "cacheReadTokens": row.cache_read_tokens,
                    "cacheReadRatio": row.cache_read_ratio,
                    "usageMissing": row.usage_missing,
                }))
                .collect::<Vec<_>>(),
            "records": state.sqlite_readers.read(|connection| {
                let total: i64 = connection
                    .query_row("SELECT count(*) FROM records", [], |row| row.get(0))
                    .map_err(|error| error.to_string())?;
                let bytes: i64 = connection
                    .query_row("SELECT COALESCE(SUM(raw_bytes), 0) FROM blobs", [], |row| row.get(0))
                    .map_err(|error| error.to_string())?;
                Ok(serde_json::json!({
                    "total": total,
                    "bytes": bytes,
                    "dbRatioOfLimit": bytes as f64 / (10.0 * 1024.0 * 1024.0 * 1024.0),
                }))
            })?,
        }
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
    #[test]
    fn exported_json_omits_free_form_credentials_and_retains_failure_category() {
        let connection = Connection::open_in_memory().unwrap();
        crate::initialize_database(&connection).unwrap();
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,error_message,failure_code,started_at) VALUES('diagnostic_fixture',?1,'conversation.respond','failed',?2,'configuration-error','1')", rusqlite::params![crate::PRIMARY_CONVERSATION_ID, "Bearer temporary-secret https://private-endpoint.example?token=another-secret provider body private-text"]).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut state = crate::test_support::app_state(connection);
        state.data_directory = directory.path().to_path_buf();
        let result = export_diagnostics(&state).unwrap();
        let encoded = fs::read_to_string(result.path).unwrap();
        for value in [
            "temporary-secret",
            "private-endpoint",
            "another-secret",
            "private-text",
        ] {
            assert!(!encoded.contains(value));
        }
        let payload: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            payload["recentRuns"][0]["failureCode"],
            "configuration-error"
        );
        assert_eq!(payload["recentRuns"][0]["status"], "failed");
    }

    #[test]
    fn dg_11_export_includes_self_diagnosis() {
        let connection = Connection::open_in_memory().unwrap();
        crate::initialize_database(&connection).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut state = crate::test_support::app_state(connection);
        state.data_directory = directory.path().to_path_buf();
        let revision = state.diagnosis.try_begin().unwrap();
        let mut report = state.diagnosis.snapshot();
        report.revision = revision;
        state.diagnosis.publish(report);
        let result = export_diagnostics(&state).unwrap();
        let payload: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(result.path).unwrap()).unwrap();
        assert!(payload["selfDiagnosis"]["revision"].is_number());
        assert_eq!(payload["selfDiagnosis"]["revision"], revision);
    }
}
