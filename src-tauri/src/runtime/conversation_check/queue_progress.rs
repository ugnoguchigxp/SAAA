//! Short spoken updates while Ornith has no final answer yet.
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::{database_error, task_queue};

pub(super) const INITIAL: &str = "少し考えます。";
pub(super) const SEARCH: &str = "只今お調べします。";
pub(super) const WAIT: &str = "少々お待ちください。";

// Persist before playback so cancellation or a TTS failure cannot erase the utterance.
pub(super) fn record_message(
    connection: &Connection,
    job: &task_queue::Job,
    text: &str,
    created_at: &str,
) -> Result<bool, String> {
    if !eligible(connection, &job.scope, &job.key)? {
        return Ok(false);
    }
    let current: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE id=?1 AND state='running' AND owner=?2 AND generation=?3)",
        params![job.id, job.owner, job.generation], |row| row.get(0),
    ).map_err(database_error)?;
    if !current {
        return Ok(false);
    }
    connection.execute(
        "INSERT OR IGNORE INTO conversation_messages(id,conversation_id,role,content,created_at)
         VALUES(?1,?2,'assistant',?3,?4)",
        params![format!("progress_{}", job.id), job.scope, text, created_at],
    ).map_err(database_error)?;
    Ok(true)
}

#[cfg(test)]
pub(super) fn enqueue_initial(
    connection: &Connection,
    scope: &str,
    key: &str,
) -> Result<(), String> {
    enqueue_initial_with_text(connection, scope, key, INITIAL)
}

pub(super) fn enqueue_initial_with_text(
    connection: &Connection,
    scope: &str,
    key: &str,
    text: &str,
) -> Result<(), String> {
    let text = if [INITIAL, SEARCH, WAIT].contains(&text) {
        text
    } else {
        WAIT
    };
    let requested_at_ms = task_queue::now_ms();
    task_queue::enqueue(
        connection,
        scope,
        "speech",
        "progress_speech",
        key,
        0,
        &json!({"text": text, "requestedAtMs": requested_at_ms}).to_string(),
        None,
    )?;
    Ok(())
}

pub(super) fn enqueue_search(
    connection: &Connection,
    scope: &str,
    key: &str,
) -> Result<(), String> {
    let already_queued: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE scope=?1 AND job_key=?2 AND kind='progress_speech')",
        params![scope, key],
        |row| row.get(0),
    ).map_err(database_error)?;
    if !already_queued {
        enqueue_initial_with_text(connection, scope, key, SEARCH)?;
    }
    Ok(())
}

pub(super) fn eligible(connection: &Connection, scope: &str, key: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM task_queue_jobs
          WHERE scope=?1 AND job_key=?2 AND lane='conversation'
            AND state IN ('queued','running'))
         AND NOT EXISTS(SELECT 1 FROM task_queue_jobs
          WHERE scope=?1 AND job_key=?2 AND kind='speech')",
            params![scope, key],
            |row| row.get(0),
        )
        .map_err(database_error)
}

pub(super) fn cancel(connection: &Connection, scope: &str, key: &str) -> Result<(), String> {
    connection.execute(
        "UPDATE task_queue_jobs SET state='cancelled',owner=NULL,lease_until_ms=NULL,updated_at_ms=?3
         WHERE scope=?1 AND job_key=?2 AND kind='progress_speech' AND state IN ('queued','running')",
        params![scope,key,task_queue::now_ms()],
    ).map_err(database_error)?;
    Ok(())
}

pub(super) fn recover(connection: &Connection, scope: &str) -> Result<(), String> {
    let now = task_queue::now_ms();
    // Old periodic announcements are retired without replaying uncertain speech.
    connection.execute(
        "UPDATE task_queue_jobs SET state='cancelled',owner=NULL,lease_until_ms=NULL,updated_at_ms=?2
         WHERE scope=?1 AND kind='progress_speech' AND generation>0 AND state IN ('queued','running','interrupted')",
        params![scope, now],
    ).map_err(database_error)?;
    connection.execute(
        "UPDATE task_queue_jobs SET state='queued',available_at_ms=max(available_at_ms,?2),updated_at_ms=?2
         WHERE scope=?1 AND kind='progress_speech' AND generation=0 AND state='interrupted' AND attempts=0",
        params![scope, now],
    ).map_err(database_error)?;
    Ok(())
}

pub(super) fn finish(connection: &Connection, job: &task_queue::Job) -> Result<(), String> {
    task_queue::finish(connection, job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_progress_is_saved_once_and_never_schedules_a_second_line() {
        let mut db = Connection::open_in_memory().unwrap();
        task_queue::migrate(&db).unwrap();
        db.execute_batch("CREATE TABLE conversation_messages(id TEXT PRIMARY KEY, conversation_id TEXT, role TEXT, content TEXT, created_at TEXT);").unwrap();
        task_queue::enqueue(&db, "c", "conversation", "user_input", "u", 0, "{}", None).unwrap();
        enqueue_initial(&db, "c", "u").unwrap();
        let initial = task_queue::claim(&mut db, "speech").unwrap().unwrap();
        assert!(record_message(&db, &initial, INITIAL, "1").unwrap());
        assert!(record_message(&db, &initial, INITIAL, "2").unwrap());
        finish(&db, &initial).unwrap();
        let count: i64 = db.query_row("SELECT count(*) FROM task_queue_jobs WHERE kind='progress_speech'", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 1);
        let saved: i64 = db.query_row("SELECT count(*) FROM conversation_messages", [], |row| row.get(0)).unwrap();
        assert_eq!(saved, 1);
    }

    #[test]
    fn recovery_discards_old_repeated_progress() {
        let db = Connection::open_in_memory().unwrap();
        task_queue::migrate(&db).unwrap();
        task_queue::enqueue(&db, "c", "speech", "progress_speech", "u", 1,
            &json!({"text":"もうすこしおまちください。"}).to_string(), None).unwrap();
        recover(&db, "c").unwrap();
        let state: String = db.query_row("SELECT state FROM task_queue_jobs WHERE generation=1", [], |row| row.get(0)).unwrap();
        assert_eq!(state, "cancelled");
    }
}
