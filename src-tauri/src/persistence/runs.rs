use rusqlite::{params, Connection};

use crate::{now_iso, runtime};

pub(crate) fn reconcile_interrupted_runs(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE runtime_runs
         SET status = 'interrupted', error_message = COALESCE(error_message, 'Application restarted'),
             failure_code = ?2,
             completed_at = ?1
         WHERE status = 'running'",
        params![
            now_iso(),
            runtime::contracts::RunFailureCode::AppRestarted.as_str()
        ],
    )?;
    connection.execute(
        r#"UPDATE provider_sessions
         SET status = 'interrupted', failure_reason = COALESCE(failure_reason, 'Application restarted'),
             release_status = CASE
               WHEN provider_kind='larm' AND allocation_id IS NOT NULL
                    AND release_status IN ('not-started','pending') THEN 'deferred-to-ttl'
               WHEN provider_kind='openai-compatible' AND release_status='not-applicable'
                    AND EXISTS (
                      SELECT 1
                      FROM settings_documents AS settings,
                           json_each(
                             CASE WHEN json_valid(settings.value_json)
                                  THEN settings.value_json
                                  ELSE '{"providers":[]}' END,
                             '$.providers'
                           ) AS configured
                      WHERE settings.namespace='providers.model' AND settings.key='default'
                        AND json_extract(configured.value, '$.id')=provider_sessions.provider_id
                        AND json_extract(configured.value, '$.kind')='gnosis'
                    ) THEN 'deferred-to-ttl'
               ELSE release_status
             END,
             updated_at = ?1
         WHERE status = 'running'"#,
        params![now_iso()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::initialize_database;
    use rusqlite::Connection;

    #[test]
    fn startup_reconciles_running_work() {
        let connection = Connection::open_in_memory().expect("in-memory sqlite");
        initialize_database(&connection).expect("migration succeeds");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
             VALUES ('conversation-1', NULL, 'conversation', 'now', 'now')",
                [],
            )
            .expect("conversation inserts");
        connection
        .execute(
            "INSERT INTO runtime_runs(id, conversation_id, route_kind, status, started_at)
             VALUES ('run-1', 'conversation-1', 'conversation.respond', 'running', 'before-restart')",
            [],
        )
        .expect("running work inserts");
        connection
            .execute(
                "INSERT INTO provider_sessions(
               id, provider_id, runtime_run_id, provider_kind, allocation_id,
               fallback_used, output_started, release_status, status, started_at, updated_at
             ) VALUES(
               'session-1','larm-primary','run-1','larm','alloc_restart',
               0,1,'pending','running','before-restart','before-restart'
             )",
                [],
            )
            .expect("running provider session inserts");
        connection
            .execute(
                "INSERT INTO provider_sessions(
                   id, provider_id, runtime_run_id, provider_kind,
                   fallback_used, output_started, release_status, status, started_at, updated_at
                 ) VALUES(
                   'session-gnosis','gnosis-qwen','run-1','openai-compatible',
                   0,0,'not-applicable','running','before-restart','before-restart'
                 )",
                [],
            )
            .expect("running gnosis session inserts");
        connection
            .execute(
                "INSERT INTO provider_sessions(
                   id, provider_id, runtime_run_id, provider_kind,
                   fallback_used, output_started, release_status, status, started_at, updated_at
                 ) VALUES(
                   'session-direct','direct-provider','run-1','openai-compatible',
                   0,0,'not-applicable','running','before-restart','before-restart'
                 )",
                [],
            )
            .expect("running direct session inserts");

        reconcile_interrupted_runs(&connection).expect("startup reconciliation succeeds");
        let (status, failure_code, supervisor_version): (String, String, Option<String>) =
            connection
                .query_row(
                    "SELECT status,failure_code,supervisor_version
             FROM runtime_runs WHERE id = 'run-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("run status loads");
        assert_eq!(status, "interrupted");
        assert_eq!(failure_code, "app-restarted");
        assert_eq!(supervisor_version, None);
        let (provider_status, release_status): (String, String) = connection
            .query_row(
                "SELECT status,release_status FROM provider_sessions WHERE id='session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("provider session loads");
        assert_eq!(provider_status, "interrupted");
        assert_eq!(release_status, "deferred-to-ttl");
        let (gnosis_status, gnosis_release_status): (String, String) = connection
            .query_row(
                "SELECT status,release_status FROM provider_sessions WHERE id='session-gnosis'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("gnosis provider session loads");
        assert_eq!(gnosis_status, "interrupted");
        assert_eq!(gnosis_release_status, "deferred-to-ttl");
        let direct_release_status: String = connection
            .query_row(
                "SELECT release_status FROM provider_sessions WHERE id='session-direct'",
                [],
                |row| row.get(0),
            )
            .expect("direct provider session loads");
        assert_eq!(direct_release_status, "not-applicable");
    }
}
