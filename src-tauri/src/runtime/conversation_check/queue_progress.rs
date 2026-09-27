//! Short spoken updates while an Ornith handoff has no final Qwen answer yet.
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::{database_error, task_queue};

pub(super) const INITIAL: &str = "少し考えます。";
pub(super) const SEARCH: &str = "お調べします。";
pub(super) const WAIT: &str = "少々お待ちください。";
pub(super) const WAITING: &str = "もうすこしおまちください。";
pub(super) const INTERVAL_MS: i64 = 10_000;
const MAX_WAITING_UPDATES: i64 = 1;

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

pub(super) fn eligible(connection: &Connection, scope: &str, key: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM task_queue_jobs
          WHERE scope=?1 AND job_key=?2 AND lane IN ('qwen','ornith')
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
    // A queued prompt has not reached a speaker. A running one may have, so never replay it.
    connection.execute(
        "UPDATE task_queue_jobs SET state='queued',available_at_ms=max(available_at_ms,?2),updated_at_ms=?2
         WHERE scope=?1 AND kind='progress_speech' AND state='interrupted' AND attempts=0",
        params![scope,now],
    ).map_err(database_error)?;
    let mut statement = connection
        .prepare(
            "SELECT DISTINCT job_key FROM task_queue_jobs
         WHERE scope=?1 AND lane IN ('qwen','ornith') AND state IN ('queued','running')",
        )
        .map_err(database_error)?;
    let keys = statement
        .query_map([scope], |row| row.get::<_, String>(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    for key in keys {
        if !eligible(connection, scope, &key)? {
            continue;
        }
        let existing: Option<i64> = connection.query_row(
            "SELECT max(generation) FROM task_queue_jobs WHERE scope=?1 AND job_key=?2 AND kind='progress_speech'",
            params![scope,key], |row| row.get(0),
        ).map_err(database_error)?;
        let Some(last_generation) = existing else {
            continue;
        };
        if last_generation >= MAX_WAITING_UPDATES {
            continue;
        }
        let pending: i64 = connection.query_row(
            "SELECT count(*) FROM task_queue_jobs WHERE scope=?1 AND job_key=?2 AND kind='progress_speech' AND state IN ('queued','running')",
            params![scope,key], |row| row.get(0),
        ).map_err(database_error)?;
        if pending != 0 {
            continue;
        }
        let id = task_queue::enqueue(
            connection,
            scope,
            "speech",
            "progress_speech",
            &key,
            last_generation + 1,
            &json!({"text":WAITING,"requestedAtMs":now}).to_string(),
            None,
        )?;
        connection
            .execute(
                "UPDATE task_queue_jobs SET available_at_ms=?2 WHERE id=?1",
                params![id, now.saturating_add(INTERVAL_MS)],
            )
            .map_err(database_error)?;
    }
    Ok(())
}

pub(super) fn finish_and_schedule(
    connection: &Connection,
    job: &task_queue::Job,
) -> Result<(), String> {
    task_queue::finish(connection, job)?;
    if job.generation >= MAX_WAITING_UPDATES || !eligible(connection, &job.scope, &job.key)? {
        return Ok(());
    }
    let requested_at_ms = serde_json::from_str::<Value>(&job.payload)
        .ok()
        .and_then(|value| value["requestedAtMs"].as_i64())
        .unwrap_or_else(task_queue::now_ms);
    let next_generation = job.generation + 1;
    let id = task_queue::enqueue(
        connection,
        &job.scope,
        "speech",
        "progress_speech",
        &job.key,
        next_generation,
        &json!({"text":WAITING,"requestedAtMs":requested_at_ms}).to_string(),
        None,
    )?;
    let now = task_queue::now_ms();
    let due = if job.generation == 0 && now < requested_at_ms.saturating_add(INTERVAL_MS) {
        requested_at_ms.saturating_add(INTERVAL_MS)
    } else {
        now.saturating_add(INTERVAL_MS)
    };
    connection
        .execute(
            "UPDATE task_queue_jobs SET available_at_ms=?2 WHERE id=?1 AND state='queued'",
            params![id, due],
        )
        .map_err(database_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_speech_uses_selected_handoff_line() {
        let db = Connection::open_in_memory().unwrap();
        task_queue::migrate(&db).unwrap();
        enqueue_initial_with_text(&db, "c", "search", SEARCH).unwrap();
        enqueue_initial_with_text(&db, "c", "invalid", "調査済みです。").unwrap();
        let mut statement = db.prepare("SELECT job_key,payload FROM task_queue_jobs WHERE kind='progress_speech' ORDER BY job_key").unwrap();
        let payloads: Vec<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(serde_json::from_str::<Value>(&payloads[0].1).unwrap()["text"], WAIT);
        assert_eq!(serde_json::from_str::<Value>(&payloads[1].1).unwrap()["text"], SEARCH);
    }

    #[test]
    fn spoken_progress_is_saved_once_and_cancelled_progress_is_not_saved() {
        let mut db = Connection::open_in_memory().unwrap();
        task_queue::migrate(&db).unwrap();
        db.execute_batch("CREATE TABLE conversation_messages(id TEXT PRIMARY KEY, conversation_id TEXT, role TEXT, content TEXT, created_at TEXT);").unwrap();
        task_queue::enqueue(&db, "c", "ornith", "ornith_task", "u", 0, "{}", None).unwrap();
        enqueue_initial(&db, "c", "u").unwrap();
        let initial = task_queue::claim(&mut db, "speech").unwrap().unwrap();
        assert!(record_message(&db, &initial, INITIAL, "1").unwrap());
        assert!(record_message(&db, &initial, INITIAL, "2").unwrap());
        let saved: (String, String, String, i64) = db
            .query_row(
                "SELECT conversation_id,role,content,count(*) FROM conversation_messages",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(saved, ("c".into(), "assistant".into(), INITIAL.into(), 1));
        finish_and_schedule(&db, &initial).unwrap();
        db.execute(
            "UPDATE task_queue_jobs SET available_at_ms=0 WHERE kind='progress_speech'",
            [],
        )
        .unwrap();
        let waiting = task_queue::claim(&mut db, "speech").unwrap().unwrap();
        cancel(&db, "c", "u").unwrap();
        assert!(!record_message(&db, &waiting, WAITING, "3").unwrap());
        let count: i64 = db
            .query_row("SELECT count(*) FROM conversation_messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn initial_and_followup_have_durable_ten_second_schedule() {
        let mut db = Connection::open_in_memory().unwrap();
        task_queue::migrate(&db).unwrap();
        task_queue::enqueue(&db, "c", "ornith", "ornith_task", "u", 0, "{}", None).unwrap();
        enqueue_initial(&db, "c", "u").unwrap();
        let initial = task_queue::claim(&mut db, "speech").unwrap().unwrap();
        assert_eq!(initial.kind, "progress_speech");
        assert_eq!(
            serde_json::from_str::<Value>(&initial.payload).unwrap()["text"],
            INITIAL
        );
        finish_and_schedule(&db, &initial).unwrap();
        let (due, created): (i64,i64) = db.query_row(
            "SELECT available_at_ms,created_at_ms FROM task_queue_jobs WHERE kind='progress_speech' AND generation=1",
            [], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert!((9_000..=10_000).contains(&(due - created)));
        assert!(task_queue::claim(&mut db, "speech").unwrap().is_none());
        db.execute(
            "UPDATE task_queue_jobs SET available_at_ms=?1 WHERE kind='progress_speech' AND generation=1",
            [task_queue::now_ms()],
        ).unwrap();
        let waiting = task_queue::claim(&mut db, "speech").unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&waiting.payload).unwrap()["text"],
            WAITING
        );
        finish_and_schedule(&db, &waiting).unwrap();
        let later: i64 = db.query_row(
            "SELECT count(*) FROM task_queue_jobs WHERE kind='progress_speech' AND generation>1",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(later, 0);
        cancel(&db, "c", "u").unwrap();
        let pending: i64 = db.query_row(
            "SELECT count(*) FROM task_queue_jobs WHERE kind='progress_speech' AND state='queued'",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(pending, 0);
    }

    #[test]
    fn restart_requeues_unstarted_prompt_but_never_replays_uncertain_audio() {
        let mut db = Connection::open_in_memory().unwrap();
        task_queue::migrate(&db).unwrap();
        task_queue::enqueue(&db, "c", "ornith", "ornith_task", "u", 0, "{}", None).unwrap();
        enqueue_initial(&db, "c", "u").unwrap();
        task_queue::recover(&db, &["ornith"], &["speech"]).unwrap();
        recover(&db, "c").unwrap();
        let initial = task_queue::claim(&mut db, "speech").unwrap().unwrap();
        assert_eq!(initial.generation, 0);
        task_queue::recover(&db, &["ornith"], &["speech"]).unwrap();
        recover(&db, "c").unwrap();
        let first_state: String = db
            .query_row(
                "SELECT state FROM task_queue_jobs WHERE kind='progress_speech' AND generation=0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(first_state, "interrupted");
        let (generation,state): (i64,String) = db.query_row(
            "SELECT generation,state FROM task_queue_jobs WHERE kind='progress_speech' AND generation=1",
            [], |row| Ok((row.get(0)?,row.get(1)?)),
        ).unwrap();
        assert_eq!((generation, state), (1, "queued".into()));
    }

    #[test]
    fn restart_does_not_resume_repeated_waiting_announcements() {
        let mut db = Connection::open_in_memory().unwrap();
        task_queue::migrate(&db).unwrap();
        task_queue::enqueue(&db, "c", "ornith", "ornith_task", "u", 0, "{}", None).unwrap();
        enqueue_initial(&db, "c", "u").unwrap();
        let initial = task_queue::claim(&mut db, "speech").unwrap().unwrap();
        finish_and_schedule(&db, &initial).unwrap();
        db.execute("UPDATE task_queue_jobs SET available_at_ms=0 WHERE kind='progress_speech' AND generation=1", []).unwrap();
        let waiting = task_queue::claim(&mut db, "speech").unwrap().unwrap();
        finish_and_schedule(&db, &waiting).unwrap();
        task_queue::recover(&db, &["ornith"], &["speech"]).unwrap();
        recover(&db, "c").unwrap();
        let count: i64 = db.query_row("SELECT count(*) FROM task_queue_jobs WHERE kind='progress_speech' AND generation>1", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
    }
}
