use super::ledger::{self, Entry, Kind, Status};
use crate::{database_error, new_id};
use rusqlite::{params, Connection};

pub(crate) fn enqueue_hold(
    connection: &Connection,
    source: &Entry,
    due_at: i64,
) -> Result<String, String> {
    let id = new_id("sched");
    ledger::insert(
        connection,
        &Entry {
            id: id.clone(),
            kind: Kind::HoldUntil,
            subject_ref: source.subject_ref.clone(),
            scope_ref: source.scope_ref.clone(),
            due_at,
            window_end_at: None,
            status: Status::Scheduled,
            origin: source.origin,
            delegation_ref: source.delegation_ref.clone(),
            revision: 1,
            supersedes: None,
            created_at: due_at,
            fired_at: None,
            fire_result: None,
            payload_id: source.payload_id.clone(),
        },
    )?;
    Ok(id)
}

pub(crate) fn pending_hold_subjects(
    connection: &Connection,
    now: i64,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT subject_ref FROM schedule_entries
             WHERE kind='hold_until' AND status='scheduled' AND due_at<=?1
             ORDER BY subject_ref LIMIT 32",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([now], |row| row.get(0))
        .map_err(database_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
}

pub(crate) fn digest_message(subjects: &[String]) -> String {
    format!("schedule-digest:{}", subjects.len())
}

pub(crate) fn record_digest(
    connection: &Connection,
    conversation_id: &str,
    subjects: &[String],
    now: i64,
) -> Result<(), String> {
    if subjects.is_empty() {
        return Ok(());
    }
    connection
        .execute(
            "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
             VALUES(?1,?2,'assistant',?3,?4)",
            params![
                new_id("message"),
                conversation_id,
                digest_message(subjects),
                now.to_string()
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::schema::migrate;
    use rusqlite::Connection;

    #[test]
    fn sl_08_digest_has_counts_not_bodies() {
        let text = digest_message(&["goal:a".into(), "task:b".into()]);
        assert_eq!(text, "schedule-digest:2");
        assert!(!text.contains("secret"));
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE conversation_messages(
                   id TEXT PRIMARY KEY, conversation_id TEXT, role TEXT, content TEXT, created_at TEXT
                 );",
            )
            .unwrap();
        migrate(&connection).unwrap();
        record_digest(&connection, "conversation_primary", &["goal:a".into()], 1).unwrap();
        let content: String = connection
            .query_row("SELECT content FROM conversation_messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(content, "schedule-digest:1");
    }
}
