use super::contracts::CodingSettings;
use crate::{database_error, now_iso};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub fn migrate(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS coding_settings(id INTEGER PRIMARY KEY CHECK(id=1), value_json TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS coding_workspaces(id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE, path TEXT NOT NULL, UNIQUE(conversation_id));
    CREATE TABLE IF NOT EXISTS coding_jobs(id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, source_id TEXT NOT NULL UNIQUE, workspace_id TEXT NOT NULL, workspace_path TEXT NOT NULL, settings_json TEXT NOT NULL, revision INTEGER NOT NULL, session_path TEXT NOT NULL UNIQUE, session_id TEXT, state TEXT NOT NULL, current_run_id TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS coding_runs(id TEXT PRIMARY KEY, job_id TEXT NOT NULL REFERENCES coding_jobs(id), source_id TEXT NOT NULL, host_run_id TEXT NOT NULL, payload TEXT NOT NULL, digest TEXT NOT NULL, delivery TEXT NOT NULL CHECK(delivery IN ('prepared','sending','accepted','rejected','unknown')), state TEXT NOT NULL CHECK(state IN ('starting','running','stopping','settled','failed','interrupted','outcome_unknown')), result_json TEXT, started_at TEXT NOT NULL, ended_at TEXT, pid INTEGER, process_identity TEXT, boundary TEXT, stop_reason TEXT);
    CREATE UNIQUE INDEX IF NOT EXISTS coding_one_run_per_source ON coding_runs(source_id);
    CREATE UNIQUE INDEX IF NOT EXISTS coding_single_active ON coding_runs((1)) WHERE state IN ('starting','running','stopping','outcome_unknown');
    CREATE TABLE IF NOT EXISTS coding_calls(source_id TEXT NOT NULL, call_id TEXT NOT NULL, digest TEXT NOT NULL, result_json TEXT NOT NULL, PRIMARY KEY(source_id,call_id));
    CREATE TABLE IF NOT EXISTS coding_origin_bindings(id TEXT PRIMARY KEY, job_id TEXT NOT NULL REFERENCES coding_jobs(id), origin_kind TEXT NOT NULL CHECK(origin_kind IN ('user_turn','delegated_event')), origin_id TEXT NOT NULL, operation_digest TEXT NOT NULL, created_at TEXT NOT NULL, UNIQUE(origin_kind,origin_id,operation_digest), UNIQUE(job_id));
    CREATE TABLE IF NOT EXISTS coding_events(sequence INTEGER PRIMARY KEY AUTOINCREMENT, job_id TEXT NOT NULL REFERENCES coding_jobs(id), run_id TEXT NOT NULL REFERENCES coding_runs(id), kind TEXT NOT NULL, data_json TEXT NOT NULL, created_at TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS coding_job_events ON coding_events(job_id,sequence);")?;
    c.execute(
        "INSERT OR IGNORE INTO coding_origin_bindings(id,job_id,origin_kind,origin_id,operation_digest,created_at)
         SELECT 'origin-' || id,id,'user_turn',source_id,'legacy:' || id,?1 FROM coding_jobs",
        [now_iso()],
    )?;
    c.execute(
        "INSERT OR IGNORE INTO coding_settings VALUES(1,?1)",
        [serde_json::to_string(&CodingSettings::default()).unwrap()],
    )?;
    Ok(())
}
pub fn settings(c: &Connection) -> Result<CodingSettings, String> {
    let text: String = c
        .query_row(
            "SELECT value_json FROM coding_settings WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    serde_json::from_str(&text).map_err(|_| "coding_settings_invalid".into())
}
pub fn enabled(c: &Connection) -> Result<bool, String> {
    Ok(settings(c)?.enabled)
}
pub fn event(c: &Connection, job: &str, run: &str, kind: &str, data: Value) -> Result<(), String> {
    c.execute(
        "INSERT INTO coding_events(job_id,run_id,kind,data_json,created_at) VALUES(?1,?2,?3,?4,?5)",
        params![job, run, kind, data.to_string(), now_iso()],
    )
    .map_err(database_error)?;
    Ok(())
}
pub fn source(c: &Connection, conversation: &str, run: &str) -> Result<String, String> {
    c.query_row("SELECT r.input_message_id FROM runtime_runs r JOIN conversation_messages m ON m.id=r.input_message_id WHERE r.id=?1 AND r.conversation_id=?2 AND r.status='running' AND m.conversation_id=?2 AND m.role IN ('user','transcript')", params![run,conversation], |r|r.get(0)).map_err(|_| "source_unavailable".into())
}
pub fn authorize(c: &Connection, job: &str, conversation: &str) -> Result<(), String> {
    // A coding job has one durable origin binding.  User-turn jobs retain the
    // old message-presence rule, while delegated jobs are authorized by their
    // still-active steward task.  The synthetic source_id used by delegated
    // runs is intentionally never treated as a conversation message.
    let valid: bool = c
        .query_row(
            "SELECT EXISTS(
        SELECT 1 FROM coding_jobs j
        LEFT JOIN coding_origin_bindings o ON o.job_id=j.id
        WHERE j.id=?1 AND j.conversation_id=?2 AND (
          (o.origin_kind='user_turn' AND EXISTS(
            SELECT 1 FROM conversation_messages m
            WHERE m.id=o.origin_id AND m.conversation_id=j.conversation_id
          ) AND NOT EXISTS(
            SELECT 1 FROM coding_runs prior
            LEFT JOIN conversation_messages source ON source.id=prior.source_id
            WHERE prior.job_id=j.id AND source.id IS NULL
          )) OR
          (o.origin_kind='delegated_event' AND EXISTS(
            SELECT 1 FROM steward_tasks t
            JOIN steward_delegations d ON d.id=t.delegation_id
            JOIN steward_goals g ON g.id=d.goal_id
            JOIN conversation_messages m ON m.id=t.source_id
            WHERE t.id=o.origin_id AND t.conversation_id=j.conversation_id
              AND t.loop_state IN ('queued','running','awaiting_user')
              AND d.status='active' AND d.superseded_by IS NULL
              AND g.status='active' AND g.superseded_by IS NULL
              AND m.conversation_id=j.conversation_id
          )) OR
          (o.job_id IS NULL AND EXISTS(
            SELECT 1 FROM conversation_messages m
            WHERE m.id=j.source_id AND m.conversation_id=j.conversation_id
          ) AND NOT EXISTS(
            SELECT 1 FROM coding_runs prior
            LEFT JOIN conversation_messages source ON source.id=prior.source_id
            WHERE prior.job_id=j.id AND source.id IS NULL
          ))
        ))",
            params![job, conversation],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if valid {
        Ok(())
    } else {
        Err("job_unavailable".into())
    }
}
pub fn inspect(
    c: &Connection,
    conversation: &str,
    job: &str,
    cursor: u64,
    limit: usize,
) -> Result<Value, String> {
    authorize(c, job, conversation)?;
    let mut value = c.query_row("SELECT j.id,j.current_run_id,j.revision,j.state,j.workspace_path,j.session_id,r.delivery,r.result_json FROM coding_jobs j JOIN coding_runs r ON r.id=j.current_run_id WHERE j.id=?1",[job],|r|Ok(json!({"jobId":r.get::<_,String>(0)?,"runId":r.get::<_,String>(1)?,"revision":r.get::<_,i64>(2)?,"state":r.get::<_,String>(3)?,"workspace":r.get::<_,String>(4)?,"sessionId":r.get::<_,Option<String>>(5)?,"delivery":r.get::<_,String>(6)?,"result":r.get::<_,Option<String>>(7)?.and_then(|s|serde_json::from_str::<Value>(&s).ok())}))).map_err(database_error)?;
    let mut stmt = c.prepare("SELECT sequence,kind,data_json FROM coding_events WHERE job_id=?1 AND sequence>?2 ORDER BY sequence LIMIT ?3").map_err(database_error)?;
    let rows = stmt.query_map(params![job,cursor,limit.min(100)+1],|r|Ok(json!({"sequence":r.get::<_,u64>(0)?,"kind":r.get::<_,String>(1)?,"data":serde_json::from_str::<Value>(&r.get::<_,String>(2)?).unwrap_or(Value::Null)}))).map_err(database_error)?;
    let mut events = Vec::new();
    let mut next = cursor;
    let mut bytes = value.to_string().len();
    let mut truncated = false;
    for row in rows {
        let row = row.map_err(database_error)?;
        bytes += row.to_string().len();
        if events.len() >= limit.min(100) || bytes > 30_000 {
            truncated = true;
            break;
        }
        next = row["sequence"].as_u64().unwrap();
        events.push(row);
    }
    value["events"] = json!(events);
    value["nextCursor"] = json!(next);
    value["truncated"] = json!(truncated);
    Ok(value)
}
pub fn revision(c: &Connection, job: &str, expected: u64) -> Result<(String, String), String> {
    let (revision, state, run): (u64, String, String) = c
        .query_row(
            "SELECT revision,state,current_run_id FROM coding_jobs WHERE id=?1",
            [job],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(database_error)?;
    if revision != expected {
        return Err("stale_revision".into());
    }
    Ok((state, run))
}
pub fn cached(
    c: &Connection,
    source: &str,
    call: &str,
    digest: &str,
) -> Result<Option<Value>, String> {
    let row: Option<(String, String)> = c
        .query_row(
            "SELECT digest,result_json FROM coding_calls WHERE source_id=?1 AND call_id=?2",
            params![source, call],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(database_error)?;
    row.map(|(d, v)| {
        if d == digest {
            serde_json::from_str(&v).map_err(|_| "ledger_invalid".into())
        } else {
            Err("idempotency_conflict".into())
        }
    })
    .transpose()
}
