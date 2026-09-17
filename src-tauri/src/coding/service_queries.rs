use crate::coding::repository as repo;
use crate::{database_error, now_iso};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub fn active_run_exists(c: &Connection) -> Result<bool, String> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM coding_runs WHERE state IN ('starting','running','stopping','outcome_unknown'))",
        [],
        |row| row.get(0),
    )
    .map_err(database_error)
}

pub fn prior_job_for_source(
    c: &Connection,
    source: &str,
) -> Result<Option<(String, String)>, String> {
    c.query_row(
        "SELECT j.id,r.digest FROM coding_jobs j JOIN coding_runs r ON r.job_id=j.id WHERE j.source_id=?1 ORDER BY r.rowid LIMIT 1",
        [source],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(database_error)
}

pub fn workspace_belongs_to_conversation(
    c: &Connection,
    workspace: Option<&str>,
    conversation: &str,
) -> Result<bool, String> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM coding_workspaces WHERE id=?1 AND conversation_id=?2)",
        params![workspace, conversation],
        |row| row.get(0),
    )
    .map_err(database_error)
}

pub fn call_count(c: &Connection, source: &str) -> Result<i64, String> {
    c.query_row(
        "SELECT COUNT(*) FROM coding_calls WHERE source_id=?1",
        [source],
        |row| row.get(0),
    )
    .map_err(database_error)
}

pub fn existing_job(c: &Connection, source: &str) -> Result<Option<String>, String> {
    c.query_row(
        "SELECT id FROM coding_jobs WHERE source_id=?1",
        [source],
        |row| row.get(0),
    )
    .optional()
    .map_err(database_error)
}

pub fn first_run_digest(c: &Connection, job: &str) -> Result<String, String> {
    c.query_row(
        "SELECT digest FROM coding_runs WHERE job_id=?1 ORDER BY rowid LIMIT 1",
        [job],
        |row| row.get(0),
    )
    .map_err(database_error)
}

pub fn workspace_path(
    c: &Connection,
    workspace: &str,
    conversation: &str,
) -> Result<String, String> {
    c.query_row(
        "SELECT path FROM coding_workspaces WHERE id=?1 AND conversation_id=?2",
        params![workspace, conversation],
        |row| row.get(0),
    )
    .map_err(|_| "workspace_required".into())
}

pub struct NewJob<'a> {
    pub id: &'a str,
    pub conversation: &'a str,
    pub source: &'a str,
    pub workspace: &'a str,
    pub workspace_path: &'a str,
    pub settings_json: &'a str,
    pub session_path: &'a str,
    pub run: &'a str,
}

pub fn insert_job(c: &Connection, job: NewJob<'_>) -> Result<(), String> {
    c.execute(
        "INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id) VALUES(?1,?2,?3,?4,?5,?6,1,?7,'queued',?8)",
        params![job.id, job.conversation, job.source, job.workspace, job.workspace_path, job.settings_json, job.session_path, job.run],
    )
    .map_err(database_error)?;
    Ok(())
}

pub fn insert_run(
    c: &Connection,
    job: &str,
    run: &str,
    source: &str,
    host: &str,
    payload: &str,
    digest: &str,
) -> Result<(), String> {
    if active_run_exists(c)? {
        return Err("busy".into());
    }
    c.execute(
        "INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at) VALUES(?1,?2,?3,?4,?5,?6,'prepared','starting',?7)",
        params![run, job, source, host, payload, digest, now_iso()],
    )
    .map_err(database_error)?;
    repo::event(c, job, run, "queued", json!({}))
}

pub fn queue_continuation(c: &Connection, job: &str, run: &str) -> Result<(), String> {
    c.execute(
        "UPDATE coding_jobs SET revision=revision+1,state='queued',current_run_id=?2 WHERE id=?1",
        params![job, run],
    )
    .map_err(database_error)?;
    Ok(())
}

pub fn record_call(
    c: &Connection,
    source: &str,
    call: &str,
    digest: &str,
    value: &Value,
) -> Result<(), String> {
    c.execute(
        "INSERT INTO coding_calls VALUES(?1,?2,?3,?4)",
        params![source, call, digest, value.to_string()],
    )
    .map_err(database_error)?;
    Ok(())
}

pub fn request_stop(c: &Connection, job: &str, run: &str, reason: &str) -> Result<(), String> {
    c.execute(
        "UPDATE coding_runs SET state='stopping',stop_reason=?2 WHERE id=?1 AND state!='outcome_unknown'",
        params![run, reason],
    )
    .map_err(database_error)?;
    c.execute(
        "UPDATE coding_jobs SET state='cancel_requested',revision=revision+1 WHERE id=?1",
        [job],
    )
    .map_err(database_error)?;
    Ok(())
}
