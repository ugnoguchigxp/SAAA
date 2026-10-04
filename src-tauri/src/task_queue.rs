//! Durable jobs with in-process wakeups. Payload interpretation belongs to each consumer.
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::database_error;

#[derive(Clone, Debug)]
pub(crate) struct Job {
    pub(crate) id: String,
    pub(crate) scope: String,
    pub(crate) lane: String,
    pub(crate) kind: String,
    pub(crate) key: String,
    pub(crate) generation: i64,
    pub(crate) payload: String,
    pub(crate) owner: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct JobStatus {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) key: String,
    pub(crate) state: String,
    pub(crate) error: Option<String>,
}

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_queue_jobs (
          id TEXT PRIMARY KEY,
          scope TEXT NOT NULL,
          lane TEXT NOT NULL,
          kind TEXT NOT NULL,
          job_key TEXT NOT NULL,
          generation INTEGER NOT NULL DEFAULT 0,
          payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
          state TEXT NOT NULL CHECK(state IN ('queued','running','completed','failed','cancelled','interrupted')),
          owner TEXT,
          attempts INTEGER NOT NULL DEFAULT 0,
          available_at_ms INTEGER NOT NULL,
          lease_until_ms INTEGER,
          error TEXT,
          created_at_ms INTEGER NOT NULL,
          updated_at_ms INTEGER NOT NULL,
          UNIQUE(scope,kind,job_key,generation)
        );
        CREATE INDEX IF NOT EXISTS idx_task_queue_ready
          ON task_queue_jobs(lane,state,available_at_ms,created_at_ms);"
    )
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn enqueue(
    connection: &Connection,
    scope: &str,
    lane: &str,
    kind: &str,
    key: &str,
    generation: i64,
    payload: &str,
    max_pending: Option<i64>,
) -> Result<String, String> {
    let prior: Option<(String, String)> = connection
        .query_row(
            "SELECT id,payload_json FROM task_queue_jobs WHERE scope=?1 AND kind=?2 AND job_key=?3 AND generation=?4",
            params![scope, kind, key, generation],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database_error)?;
    if let Some((id, prior_payload)) = prior {
        if prior_payload == payload {
            return Ok(id);
        }
        return Err("同じ入力IDで異なる内容は登録できません。".into());
    }
    if let Some(max_pending) = max_pending {
        let pending: i64 = connection
            .query_row(
                "SELECT count(*) FROM task_queue_jobs WHERE scope=?1 AND lane=?2 AND kind=?3 AND state IN ('queued','running')",
                params![scope,lane,kind],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if pending >= max_pending {
            return Err("会話の処理キューが満杯です。少し待ってから再送してください。".into());
        }
    }
    let id = format!("queue_{}", uuid::Uuid::new_v4().simple());
    let now = now_ms();
    connection
        .execute(
            "INSERT INTO task_queue_jobs(id,scope,lane,kind,job_key,generation,payload_json,state,available_at_ms,created_at_ms,updated_at_ms)
             VALUES(?1,?2,?3,?4,?5,?6,?7,'queued',?8,?8,?8)",
            params![id, scope, lane, kind, key, generation, payload, now],
        )
        .map_err(database_error)?;
    Ok(id)
}

pub(crate) fn claim(connection: &mut Connection, lane: &str) -> Result<Option<Job>, String> {
    let transaction = connection.transaction().map_err(database_error)?;
    let now = now_ms();
    let candidate: Option<(String, String, String, String, String, i64, String)> = transaction
        .query_row(
            "SELECT id,scope,lane,kind,job_key,generation,payload_json FROM task_queue_jobs
             WHERE lane=?1 AND state='queued' AND available_at_ms<=?2
             ORDER BY CASE WHEN kind='terminal_question' THEN 1 ELSE 0 END,rowid LIMIT 1",
            params![lane, now],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some((id, scope, lane, kind, key, generation, payload)) = candidate else {
        transaction.commit().map_err(database_error)?;
        return Ok(None);
    };
    let owner = uuid::Uuid::new_v4().simple().to_string();
    let changed = transaction
        .execute(
            "UPDATE task_queue_jobs SET state='running',owner=?2,attempts=attempts+1,lease_until_ms=?3,updated_at_ms=?4
             WHERE id=?1 AND state='queued'",
            params![id, owner, now + 180_000, now],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("キューの仕事を取得できませんでした。".into());
    }
    transaction.commit().map_err(database_error)?;
    Ok(Some(Job {
        id,
        scope,
        lane,
        kind,
        key,
        generation,
        payload,
        owner,
    }))
}

pub(crate) fn finish(connection: &Connection, job: &Job) -> Result<(), String> {
    let changed = connection
        .execute(
            "UPDATE task_queue_jobs SET state='completed',owner=NULL,lease_until_ms=NULL,updated_at_ms=?4
             WHERE id=?1 AND state='running' AND owner=?2 AND generation=?3",
            params![job.id, job.owner, job.generation, now_ms()],
        )
        .map_err(database_error)?;
    if changed == 1 {
        Ok(())
    } else {
        Err("期限切れのキュー仕事は完了できません。".into())
    }
}

pub(crate) fn fail(connection: &Connection, job: &Job, error: &str) -> Result<(), String> {
    // Retrying the same credential or malformed contract cannot repair it.
    if error == "Provider authentication failed. Check the configured credential."
        || error.contains("larm_invalid_provider")
        || error.contains("larm_invalid_credential")
    {
        return fail_terminal(connection, job, error);
    }
    let error = error.chars().take(500).collect::<String>();
    let changed = connection
        .execute(
            "UPDATE task_queue_jobs SET state=CASE WHEN attempts<3 THEN 'queued' ELSE 'failed' END,
             owner=NULL,lease_until_ms=NULL,available_at_ms=?4+(1000*attempts),error=?5,updated_at_ms=?4
             WHERE id=?1 AND state='running' AND owner=?2 AND generation=?3",
            params![job.id, job.owner, job.generation, now_ms(), error],
        )
        .map_err(database_error)?;
    if changed == 1 {
        Ok(())
    } else {
        Err("期限切れのキュー仕事は更新できません。".into())
    }
}

pub(crate) fn fail_terminal(connection: &Connection, job: &Job, error: &str) -> Result<(), String> {
    let changed = connection
        .execute(
            "UPDATE task_queue_jobs SET state='failed',owner=NULL,lease_until_ms=NULL,error=?4,updated_at_ms=?5
             WHERE id=?1 AND state='running' AND owner=?2 AND generation=?3",
            params![job.id, job.owner, job.generation, error.chars().take(500).collect::<String>(), now_ms()],
        )
        .map_err(database_error)?;
    if changed == 1 {
        Ok(())
    } else {
        Err("期限切れのキュー仕事は更新できません。".into())
    }
}

pub(crate) fn recover(
    connection: &Connection,
    replay_lanes: &[&str],
    interrupt_lanes: &[&str],
) -> Result<(), String> {
    let now = now_ms();
    for lane in replay_lanes {
        connection.execute(
            "UPDATE task_queue_jobs SET state='queued',owner=NULL,lease_until_ms=NULL,available_at_ms=?1,updated_at_ms=?1
             WHERE state='running' AND lane=?2",
            params![now,lane],
        ).map_err(database_error)?;
    }
    for lane in interrupt_lanes {
        connection.execute(
            "UPDATE task_queue_jobs SET state='interrupted',owner=NULL,lease_until_ms=NULL,updated_at_ms=?1
             WHERE state IN ('queued','running') AND lane=?2",
            params![now,lane],
        ).map_err(database_error)?;
    }
    Ok(())
}

pub(crate) fn cancel_key(connection: &Connection, scope: &str, key: &str) -> Result<usize, String> {
    connection.execute(
        "UPDATE task_queue_jobs SET state='cancelled',owner=NULL,lease_until_ms=NULL,updated_at_ms=?3
         WHERE scope=?1 AND job_key=?2 AND state IN ('queued','running')",
        params![scope,key,now_ms()],
    ).map_err(database_error)
}

pub(crate) fn snapshot(connection: &Connection, scope: &str) -> Result<Vec<JobStatus>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id,kind,job_key,state,error FROM task_queue_jobs WHERE scope=?1
         AND (state IN ('queued','running') OR rowid IN
              (SELECT rowid FROM task_queue_jobs WHERE scope=?1 ORDER BY rowid DESC LIMIT 100))
         ORDER BY rowid",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([scope], |row| {
            Ok(JobStatus {
                id: row.get(0)?,
                kind: row.get(1)?,
                key: row.get(2)?,
                state: row.get(3)?,
                error: row.get(4)?,
            })
        })
        .map_err(database_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deduplicates_and_recovers_claims() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        let id = enqueue(
            &connection,
            "c",
            "qwen",
            "user_input",
            "u",
            0,
            "{\"text\":\"hello\"}",
            Some(16),
        )
        .unwrap();
        assert_eq!(
            id,
            enqueue(
                &connection,
                "c",
                "qwen",
                "user_input",
                "u",
                0,
                "{\"text\":\"hello\"}",
                Some(16)
            )
            .unwrap()
        );
        assert!(enqueue(
            &connection,
            "c",
            "qwen",
            "user_input",
            "u",
            0,
            "{\"text\":\"different\"}",
            Some(16)
        )
        .is_err());
        let job = claim(&mut connection, "qwen").unwrap().unwrap();
        assert!(claim(&mut connection, "qwen").unwrap().is_none());
        recover(&connection, &["qwen"], &["speech"]).unwrap();
        assert!(finish(&connection, &job).is_err());
        let claimed = claim(&mut connection, "qwen").unwrap().unwrap();
        finish(&connection, &claimed).unwrap();
        assert_eq!(snapshot(&connection, "c").unwrap()[0].state, "completed");
    }

    #[test]
    fn cancelled_claim_cannot_publish_a_downstream_job() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        enqueue(&connection, "c", "ornith", "research", "u", 0, "{}", None).unwrap();
        let claimed = claim(&mut connection, "ornith").unwrap().unwrap();
        cancel_key(&connection, "c", "u").unwrap();
        {
            let transaction = connection.transaction().unwrap();
            enqueue(&transaction, "c", "qwen", "result", "u", 0, "{}", None).unwrap();
            assert!(finish(&transaction, &claimed).is_err());
            // Failed completion drops the transaction, including its downstream enqueue.
        }
        assert!(claim(&mut connection, "qwen").unwrap().is_none());
        assert_eq!(snapshot(&connection, "c").unwrap()[0].state, "cancelled");
    }

    #[test]
    fn capacity_and_retry_are_bounded_and_speech_is_not_replayed_on_startup() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        enqueue(&connection, "c", "qwen", "input", "one", 0, "{}", Some(1)).unwrap();
        assert!(enqueue(&connection, "c", "qwen", "input", "two", 0, "{}", Some(1)).is_err());
        let claimed = claim(&mut connection, "qwen").unwrap().unwrap();
        for attempt in 0..3 {
            if attempt > 0 {
                connection
                    .execute(
                        "UPDATE task_queue_jobs SET available_at_ms=0 WHERE job_key='one'",
                        [],
                    )
                    .unwrap();
            }
            let current = if attempt == 0 {
                claimed.clone()
            } else {
                claim(&mut connection, "qwen").unwrap().unwrap()
            };
            fail(&connection, &current, "provider unavailable").unwrap();
        }
        assert_eq!(snapshot(&connection, "c").unwrap()[0].state, "failed");
        enqueue(&connection, "c", "speech", "speech", "one", 0, "{}", None).unwrap();
        recover(&connection, &["qwen"], &["speech"]).unwrap();
        assert!(claim(&mut connection, "speech").unwrap().is_none());
        assert_eq!(snapshot(&connection, "c").unwrap()[1].state, "interrupted");
    }

    #[test]
    fn migration_on_isolated_existing_database_preserves_provider_settings() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE settings_documents(namespace TEXT,key TEXT,value_json TEXT,PRIMARY KEY(namespace,key));
             INSERT INTO settings_documents VALUES('providers.model','default','{\"provider\":\"saved\"}');"
        ).unwrap();
        migrate(&connection).unwrap();
        migrate(&connection).unwrap();
        let saved: String = connection.query_row(
            "SELECT value_json FROM settings_documents WHERE namespace='providers.model' AND key='default'",
            [], |row| row.get(0)
        ).unwrap();
        assert_eq!(saved, "{\"provider\":\"saved\"}");
    }

    #[test]
    fn authentication_failure_does_not_retry_the_same_credential() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        enqueue(
            &connection,
            "c",
            "ornith",
            "ornith_task",
            "u",
            0,
            "{}",
            None,
        )
        .unwrap();
        let job = claim(&mut connection, "ornith").unwrap().unwrap();
        fail(
            &connection,
            &job,
            "Provider authentication failed. Check the configured credential.",
        )
        .unwrap();
        assert_eq!(snapshot(&connection, "c").unwrap()[0].state, "failed");
        assert!(claim(&mut connection, "ornith").unwrap().is_none());
    }

    #[test]
    fn terminal_failure_is_not_reclaimed() {
        let mut connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        enqueue(&connection, "c", "qwen", "result", "u", 0, "{}", None).unwrap();
        let job = claim(&mut connection, "qwen").unwrap().unwrap();
        fail_terminal(&connection, &job, "audio started").unwrap();
        assert!(claim(&mut connection, "qwen").unwrap().is_none());
        assert_eq!(snapshot(&connection, "c").unwrap()[0].state, "failed");
    }
}
