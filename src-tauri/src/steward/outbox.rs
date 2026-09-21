//! Report claim and conversation message insertion share one transaction.
use super::repository as repo;
use crate::{database_error, new_id, now_iso};
use rusqlite::{params, Connection};

pub(crate) fn flush_unflushed(
    connection: &Connection,
    conversation_id: &str,
    now_ms: i64,
) -> Result<Option<String>, String> {
    crate::steward::faults::maybe("report_before_message")?;
    let Some(digest) = repo::unflushed_digest(connection, conversation_id, now_ms)? else {
        return Ok(None);
    };
    crate::steward::faults::maybe("message_insert")?;
    let message_id = new_id("message");
    connection
        .execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES(?1,?2,'assistant',?3,?4)",
            params![message_id, conversation_id, digest, now_iso()],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "UPDATE conversations SET updated_at=?1 WHERE id=?2",
            params![now_iso(), conversation_id],
        )
        .map_err(database_error)?;
    crate::steward::faults::maybe("mark_flushed")?;
    repo::mark_flushed(connection, conversation_id, now_ms, &message_id)?;
    connection
        .execute(
            "INSERT INTO steward_delivery_cursor(conversation_id,revision,last_message_id,updated_at)
             VALUES(?1,1,?2,?3)
             ON CONFLICT(conversation_id) DO UPDATE SET
               revision=revision+1,last_message_id=excluded.last_message_id,updated_at=excluded.updated_at",
            params![conversation_id, message_id, now_iso()],
        )
        .map_err(database_error)?;
    Ok(Some(message_id))
}
