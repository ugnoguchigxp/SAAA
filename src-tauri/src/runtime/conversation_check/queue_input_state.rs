//! Input ordering and cancellation shared by explicit cancellation and replacement.
use rusqlite::{params, Connection, OptionalExtension};

use crate::{database_error, task_queue};

pub(crate) fn active_previous(
    connection: &Connection,
    scope: &str,
    current_key: &str,
) -> Result<Option<(String, String)>, String> {
    connection.query_row(
        "SELECT input.job_key,m.content FROM task_queue_jobs input
         JOIN conversation_messages m ON m.id='check_' || input.job_key
           AND m.conversation_id=input.scope AND m.role='user'
         WHERE input.scope=?1 AND input.kind='user_input'
           AND input.rowid < (SELECT rowid FROM task_queue_jobs
                             WHERE scope=?1 AND kind='user_input' AND job_key=?2)
           AND EXISTS (SELECT 1 FROM task_queue_jobs active
                       WHERE active.scope=input.scope AND active.job_key=input.job_key
                         AND active.lane IN ('qwen','ornith') AND active.state IN ('queued','running'))
         ORDER BY input.rowid DESC LIMIT 1",
        params![scope, current_key], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(database_error)
}

/// Call inside the transaction that accepts the cancellation/replacement.
pub(crate) fn cancel(
    connection: &Connection,
    scope: &str,
    key: &str,
    completed_at: &str,
) -> Result<(), String> {
    task_queue::cancel_key(connection, scope, key)?;
    connection
        .execute(
            "UPDATE runtime_runs SET status='cancelled',completed_at=?3
         WHERE id=?1 AND conversation_id=?2 AND status='running'",
            params![format!("run_{key}"), scope, completed_at],
        )
        .map_err(database_error)?;
    Ok(())
}
