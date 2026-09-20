use super::repository as repo;
use crate::situation::speech_holds_tts;
use crate::{database_error, new_id, now_iso, AppState};
use rusqlite::{params, Connection};

pub(crate) fn publish(
    state: &AppState,
    connection: &Connection,
    conversation_id: &str,
) -> Result<(), String> {
    repo::sync_from_coding(connection, conversation_id)?;
    queue_terminals(
        state,
        connection,
        conversation_id,
        &repo::claim_terminals(connection, conversation_id)?,
    )
}

pub(crate) fn queue_terminals(
    state: &AppState,
    connection: &Connection,
    conversation_id: &str,
    terminals: &[String],
) -> Result<(), String> {
    if !terminals.is_empty() {
        repo::enqueue_report(
            connection,
            conversation_id,
            &terminals.join("\n"),
            speech_holds_tts(state).then_some("meeting"),
        )?;
    }
    if !speech_holds_tts(state) {
        flush_unflushed(connection, conversation_id)?;
    }
    Ok(())
}

pub(crate) fn flush_held_reports(state: &AppState, conversation_id: &str) -> Result<(), String> {
    if speech_holds_tts(state) {
        return Ok(());
    }
    state.sqlite_writer.write(|connection| {
        publish(state, connection, conversation_id)?;
        flush_unflushed(connection, conversation_id)
    })
}

fn flush_unflushed(connection: &Connection, conversation_id: &str) -> Result<(), String> {
    let Some(digest) = repo::unflushed_digest(connection, conversation_id)? else {
        return Ok(());
    };
    connection
        .execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES(?1,?2,'assistant',?3,?4)",
            params![new_id("message"), conversation_id, digest, now_iso()],
        )
        .map_err(database_error)?;
    connection
        .execute(
            "UPDATE conversations SET updated_at=?1 WHERE id=?2",
            params![now_iso(), conversation_id],
        )
        .map_err(database_error)?;
    repo::mark_flushed(connection, conversation_id)
}
