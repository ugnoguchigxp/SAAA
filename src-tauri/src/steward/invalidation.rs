//! Forget invalidates undelivered reports, speech, and derived summaries.
use crate::{database_error, now_iso};
use rusqlite::{params, Connection};

pub(crate) fn forget_source(connection: &Connection, source_id: &str) -> Result<(), String> {
    super::repository::forget_source(connection, source_id)?;
    connection
        .execute(
            "UPDATE steward_reports SET invalidated=1,digest='',content_json=NULL,held_reason='forgotten'
             WHERE flushed=0 AND task_id IN (SELECT id FROM steward_tasks WHERE source_id=?1)",
            [source_id],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "UPDATE steward_goals SET summary='' WHERE id IN (
               SELECT d.goal_id FROM steward_delegations d
               JOIN steward_tasks t ON t.delegation_id=d.id WHERE t.source_id=?1)",
            [source_id],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "UPDATE conversation_messages SET content='' WHERE id IN (
               SELECT message_id FROM steward_reports WHERE invalidated=1 AND message_id IS NOT NULL
               AND task_id IN (SELECT id FROM steward_tasks WHERE source_id=?1))",
            [source_id],
        )
        .map_err(database_error)?;
    let _ = now_iso();
    let _ = params![source_id];
    Ok(())
}
