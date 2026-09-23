use super::{cancel, queries};
use crate::coding::{contracts::*, repository as repo};
use crate::{new_id, AppState, StartTurnInput};
use rusqlite::Connection;
use serde_json::{json, Value};

pub struct ToolTransaction<'a> {
    pub state: &'a AppState,
    pub tx: &'a Connection,
    pub input: &'a StartTurnInput,
    pub call: &'a crate::runtime::agent_tools::AgentToolCall,
    pub canonical: &'a Value,
    pub digest: &'a str,
    pub source: &'a str,
}

impl ToolTransaction<'_> {
    pub fn execute(self, launch: &mut Option<String>) -> Result<Value, String> {
        let settings = repo::settings(self.tx)?;
        match self.call.name.as_str() {
            "coding_start" => start(&self, &settings, launch),
            "coding_inspect" => inspect(self.tx, self.input, self.canonical),
            "coding_continue" => continue_job(&self, &settings, launch),
            "coding_cancel" => cancel_job(self.tx, self.input, self.canonical),
            _ => Err("invalid_tool".into()),
        }
    }
}

fn start(
    context: &ToolTransaction<'_>,
    settings: &CodingSettings,
    launch: &mut Option<String>,
) -> Result<Value, String> {
    let args: Start =
        serde_json::from_value(context.canonical.clone()).map_err(|_| "invalid_arguments")?;
    let tx = context.tx;
    let input = context.input;
    if !settings.enabled {
        return Err("coding_disabled".into());
    }
    if let Some(job) = queries::existing_job(tx, context.source)? {
        if queries::first_run_digest(tx, &job)? != context.digest {
            return Err("idempotency_conflict".into());
        }
        let mut value = repo::inspect(tx, &input.conversation_id, &job, 0, 1)?;
        value["accepted"] = json!(true);
        return Ok(value);
    }

    let path = queries::workspace_path(tx, &args.workspace_id, &input.conversation_id)?;
    let actual = std::fs::canonicalize(&path).map_err(|_| "workspace_missing")?;
    if actual.to_str() != Some(path.as_str()) || !actual.join(".git").exists() {
        return Err("workspace_invalid".into());
    }
    let job = new_id("coding");
    let run = new_id("coding_run");
    let directory = context.state.data_directory.join("coding-sessions");
    std::fs::create_dir_all(&directory).map_err(|_| "session_storage_unavailable")?;
    let session = directory.join(format!("{job}.jsonl"));
    let settings_json = serde_json::to_string(settings).map_err(|_| "coding_settings_invalid")?;
    let session_path = session.to_string_lossy();
    queries::insert_job(
        tx,
        queries::NewJob {
            id: &job,
            conversation: &input.conversation_id,
            source: context.source,
            workspace: &args.workspace_id,
            workspace_path: &path,
            settings_json: &settings_json,
            session_path: &session_path,
            run: &run,
        },
    )?;
    let workspace_scope =
        crate::runtime::context::scope::register(tx, "resource", &args.workspace_id)?;
    let job_scope = crate::runtime::context::scope::register(tx, "task", &job)?;
    crate::runtime::context::scope::link(tx, &workspace_scope, &job_scope)?;
    queries::insert_run(
        tx,
        &job,
        &run,
        context.source,
        &input.run_id,
        &args.request,
        context.digest,
    )?;
    *launch = Some(run.clone());
    Ok(json!({"jobId":job,"runId":run,"revision":1,"state":"queued","accepted":true}))
}

fn inspect(tx: &Connection, input: &StartTurnInput, canonical: &Value) -> Result<Value, String> {
    let args: Inspect =
        serde_json::from_value(canonical.clone()).map_err(|_| "invalid_arguments")?;
    repo::inspect(
        tx,
        &input.conversation_id,
        &args.job_id,
        args.cursor,
        args.limit,
    )
}

fn continue_job(
    context: &ToolTransaction<'_>,
    settings: &CodingSettings,
    launch: &mut Option<String>,
) -> Result<Value, String> {
    let args: Continue =
        serde_json::from_value(context.canonical.clone()).map_err(|_| "invalid_arguments")?;
    let tx = context.tx;
    let input = context.input;
    if !settings.enabled {
        return Err("coding_disabled".into());
    }
    repo::authorize(tx, &args.job_id, &input.conversation_id)?;
    let (status, _) = repo::revision(tx, &args.job_id, args.expected_revision)?;
    if !matches!(status.as_str(), "settled" | "failed" | "interrupted") {
        return Err("busy".into());
    }
    let (saved_settings, delivery): (String, String) = tx
        .query_row(
            "SELECT j.settings_json,r.delivery FROM coding_jobs j JOIN coding_runs r ON r.id=j.current_run_id WHERE j.id=?1",
            [&args.job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(crate::database_error)?;
    if delivery == "unknown"
        && serde_json::from_str::<CodingSettings>(&saved_settings)
            .map_err(|_| "coding_settings_invalid")?
            .implementation_method == "codex-sdk"
    {
        return Err("coding_outcome_unknown".into());
    }
    let run = new_id("coding_run");
    queries::insert_run(
        tx,
        &args.job_id,
        &run,
        context.source,
        &input.run_id,
        &args.request,
        context.digest,
    )?;
    queries::queue_continuation(tx, &args.job_id, &run)?;
    *launch = Some(run.clone());
    Ok(
        json!({"jobId":args.job_id,"runId":run,"revision":args.expected_revision+1,"state":"queued","accepted":true}),
    )
}

fn cancel_job(tx: &Connection, input: &StartTurnInput, canonical: &Value) -> Result<Value, String> {
    let args: Cancel =
        serde_json::from_value(canonical.clone()).map_err(|_| "invalid_arguments")?;
    cancel(
        tx,
        &input.conversation_id,
        &args.job_id,
        args.expected_revision,
        &args.reason,
    )
}
