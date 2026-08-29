use crate::database_error;
use rusqlite::Connection;
use serde_json::{json, Value};

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
