use crate::{database_error, new_id, PRIMARY_CONVERSATION_ID};
use rusqlite::{params, Connection};

pub(crate) fn assistant(connection: &Connection, body: &str, now: i64) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
             VALUES(?1,?2,'assistant',?3,?4)",
            params![
                new_id("message"),
                PRIMARY_CONVERSATION_ID,
                body,
                now.to_string()
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn candidate(subject_ref: &str) -> String {
    format!("schedule-ask:{subject_ref}")
}

pub(crate) fn started(subject_ref: &str) -> String {
    format!("schedule-started:{subject_ref}")
}

pub(crate) fn inspect(subject_ref: &str) -> String {
    format!("schedule-inspect:{subject_ref}")
}

pub(crate) fn conflict(subject_ref: &str) -> String {
    format!("schedule-conflict:{subject_ref}")
}

pub(crate) fn foreign(event_id: &str) -> String {
    format!("schedule-foreign:{event_id}")
}

pub(crate) fn calendar_error(code: &str) -> String {
    format!("schedule-calendar:{code}")
}

pub(crate) fn confirm_delete(subject_ref: &str) -> String {
    format!("schedule-confirm-delete:{subject_ref}")
}
