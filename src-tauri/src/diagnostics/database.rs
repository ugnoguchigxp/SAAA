use rusqlite::Connection;
use serde_json::{json, Value};

use crate::{database_error, persistence::SqliteReaders, redact::redact_runtime_text};

pub(super) struct DatabaseDiagnostics {
    pub(super) database: Value,
    pub(super) situation: Value,
    pub(super) recent_runs: Vec<Value>,
    pub(super) provider_sessions: Value,
    pub(super) audit_trail: Vec<Value>,
}

pub(super) fn load(readers: &SqliteReaders) -> Result<DatabaseDiagnostics, String> {
    readers.read(|connection| {
        let settings = crate::persistence::list_settings_documents(connection)?
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
                "SELECT route_kind, COALESCE(provider_id, ''), status, COALESCE(error_message, ''),
                        COALESCE(failure_code, '')
                 FROM runtime_runs ORDER BY started_at DESC LIMIT 20",
            )
            .map_err(database_error)?;
        let recent_runs = statement
            .query_map([], |row| {
                Ok(json!({
                    "route": row.get::<_, String>(0)?,
                    "providerId": row.get::<_, String>(1)?,
                    "status": row.get::<_, String>(2)?,
                    "error": redact_runtime_text(&row.get::<_, String>(3)?),
                    "failureCode": row.get::<_, String>(4)?
                }))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        drop(statement);
        let evaluation = crate::situation::repository::evaluation_summary(connection)?;
        let settings_document = crate::situation::repository::load_settings(connection)?;
        let profile = crate::situation::calibration::active_profile(connection)?;
        let calibration = crate::situation::calibration::latest_run(connection)?;
        Ok(DatabaseDiagnostics {
            database: json!({
                "settingsDocuments": settings,
                "conversationCount": conversation_count,
                "messageCount": message_count
            }),
            situation: json!({
                "monitoringEnabled": settings_document.enabled,
                "calendarEnabled": settings_document.calendar_enabled,
                "activeRuleVersion": profile.rule_version,
                "latestCalibrationStatus": calibration.as_ref().map(|run| run.status.as_str()),
                "totalEntries": evaluation.total_entries,
                "feedback": {
                    "accurate": evaluation.accurate,
                    "inaccurate": evaluation.inaccurate,
                    "unsure": evaluation.unsure
                }
            }),
            recent_runs,
            provider_sessions: build_provider_diagnostics(connection)?,
            audit_trail: crate::persistence::audit::recent_events(connection, 1_000)?,
        })
    })
}

pub(super) fn build_provider_diagnostics(connection: &Connection) -> Result<Value, String> {
    let mut statement = connection
        .prepare(
            "SELECT COALESCE(provider_kind, 'openai-compatible'), COALESCE(route_id, ''),
                    COALESCE(selected_runtime_id, ''), COALESCE(fallback_used, 0),
                    COALESCE(selection_reason, ''), status, COALESCE(failure_kind, ''),
                    release_status, COALESCE(release_failure_kind, '')
             FROM provider_sessions ORDER BY updated_at DESC LIMIT 20",
        )
        .map_err(database_error)?;
    let recent = statement
        .query_map([], |row| {
            Ok(json!({
                "providerKind": row.get::<_, String>(0)?, "route": row.get::<_, String>(1)?,
                "runtime": row.get::<_, String>(2)?, "fallbackUsed": row.get::<_, bool>(3)?,
                "selectionReason": row.get::<_, String>(4)?, "status": row.get::<_, String>(5)?,
                "failureKind": row.get::<_, String>(6)?, "releaseStatus": row.get::<_, String>(7)?,
                "releaseFailureKind": row.get::<_, String>(8)?
            }))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    let mut statement = connection
        .prepare(
            "SELECT COALESCE(provider_kind, 'openai-compatible'), COALESCE(route_id, ''),
                    COALESCE(selected_runtime_id, ''), COALESCE(fallback_used, 0),
                    COALESCE(failure_kind, ''), release_status, COUNT(*)
             FROM provider_sessions
             GROUP BY provider_kind, route_id, selected_runtime_id, fallback_used,
                      failure_kind, release_status ORDER BY COUNT(*) DESC LIMIT 50",
        )
        .map_err(database_error)?;
    let aggregates = statement
        .query_map([], |row| {
            Ok(json!({
                "providerKind": row.get::<_, String>(0)?, "route": row.get::<_, String>(1)?,
                "runtime": row.get::<_, String>(2)?, "fallbackUsed": row.get::<_, bool>(3)?,
                "failureKind": row.get::<_, String>(4)?, "releaseStatus": row.get::<_, String>(5)?,
                "count": row.get::<_, i64>(6)?
            }))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(json!({ "recent": recent, "aggregates": aggregates }))
}
