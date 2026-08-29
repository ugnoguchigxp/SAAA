use crate::database_error;
use crate::now_iso;
use crate::persistence::list_settings_documents;
use crate::redact::redact_runtime_text;
use crate::{AppState, LocalArtifactResult};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::env;
use std::fs;

pub(super) fn build_provider_diagnostics(connection: &Connection) -> Result<Value, String> {
    let mut provider_statement = connection
        .prepare(
            "SELECT COALESCE(provider_kind, 'openai-compatible'), COALESCE(route_id, ''),
                    COALESCE(selected_runtime_id, ''), COALESCE(fallback_used, 0),
                    COALESCE(selection_reason, ''), status, COALESCE(failure_kind, ''),
                    release_status, COALESCE(release_failure_kind, '')
             FROM provider_sessions ORDER BY updated_at DESC LIMIT 20",
        )
        .map_err(database_error)?;
    let recent = provider_statement
        .query_map([], |row| {
            Ok(json!({
                "providerKind": row.get::<_, String>(0)?,
                "route": row.get::<_, String>(1)?,
                "runtime": row.get::<_, String>(2)?,
                "fallbackUsed": row.get::<_, bool>(3)?,
                "selectionReason": row.get::<_, String>(4)?,
                "status": row.get::<_, String>(5)?,
                "failureKind": row.get::<_, String>(6)?,
                "releaseStatus": row.get::<_, String>(7)?,
                "releaseFailureKind": row.get::<_, String>(8)?
            }))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(provider_statement);
    let mut aggregate_statement = connection
        .prepare(
            "SELECT COALESCE(provider_kind, 'openai-compatible'), COALESCE(route_id, ''),
                    COALESCE(selected_runtime_id, ''), COALESCE(fallback_used, 0),
                    COALESCE(failure_kind, ''), release_status, COUNT(*)
             FROM provider_sessions
             GROUP BY provider_kind, route_id, selected_runtime_id, fallback_used,
                      failure_kind, release_status
             ORDER BY COUNT(*) DESC LIMIT 50",
        )
        .map_err(database_error)?;
    let aggregates = aggregate_statement
        .query_map([], |row| {
            Ok(json!({
                "providerKind": row.get::<_, String>(0)?,
                "route": row.get::<_, String>(1)?,
                "runtime": row.get::<_, String>(2)?,
                "fallbackUsed": row.get::<_, bool>(3)?,
                "failureKind": row.get::<_, String>(4)?,
                "releaseStatus": row.get::<_, String>(5)?,
                "count": row.get::<_, i64>(6)?
            }))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(json!({ "recent": recent, "aggregates": aggregates }))
}

pub(crate) fn export_diagnostics(state: &AppState) -> Result<LocalArtifactResult, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let settings = list_settings_documents(&connection)?
        .into_iter()
        .map(|document| {
            json!({
                "namespace": document.namespace,
                "key": document.key,
                "schemaVersion": document.schema_version,
                "updatedAt": document.updated_at
            })
        })
        .collect::<Vec<_>>();
    let conversation_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))
        .map_err(database_error)?;
    let message_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM conversation_messages", [], |row| {
            row.get(0)
        })
        .map_err(database_error)?;
    let mut statement = connection
        .prepare(
            "SELECT route_kind, COALESCE(provider_id, ''), status, COALESCE(error_message, '')
             FROM runtime_runs ORDER BY started_at DESC LIMIT 20",
        )
        .map_err(database_error)?;
    let recent_runs = statement
        .query_map([], |row| {
            Ok(json!({
                "route": row.get::<_, String>(0)?,
                "providerId": row.get::<_, String>(1)?,
                "status": row.get::<_, String>(2)?,
                "error": redact_runtime_text(&row.get::<_, String>(3)?)
            }))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    let provider_diagnostics = build_provider_diagnostics(&connection)?;
    let situation_evaluation = crate::situation::repository::evaluation_summary(&connection)?;
    let situation_settings = crate::situation::repository::load_settings(&connection)?;
    let situation_profile = crate::situation::calibration::active_profile(&connection)?;
    let latest_calibration = crate::situation::calibration::latest_run(&connection)?;
    drop(connection);

    let created_at = now_iso();
    let payload = json!({
        "format": "saaa-diagnostics-v1",
        "createdAt": created_at,
        "redacted": true,
        "application": { "version": env!("CARGO_PKG_VERSION"), "platform": env::consts::OS, "arch": env::consts::ARCH },
        "database": { "settingsDocuments": settings, "conversationCount": conversation_count, "messageCount": message_count },
        "situation": {
            "monitoringEnabled": situation_settings.enabled,
            "calendarEnabled": situation_settings.calendar_enabled,
            "activeRuleVersion": situation_profile.rule_version,
            "latestCalibrationStatus": latest_calibration.as_ref().map(|run| run.status.as_str()),
            "totalEntries": situation_evaluation.total_entries,
            "feedback": {
                "accurate": situation_evaluation.accurate,
                "inaccurate": situation_evaluation.inaccurate,
                "unsure": situation_evaluation.unsure
            }
        },
        "recentRuns": recent_runs,
        "providerSessions": provider_diagnostics
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
