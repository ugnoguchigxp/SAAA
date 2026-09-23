use super::{contracts::*, repository as repo};
use crate::{database_error, new_id, now_iso, AppState, StartTurnInput};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};

#[path = "service_queries.rs"]
mod queries;
#[path = "service_transactions.rs"]
mod transactions;
#[path = "workspace.rs"]
mod workspace;

pub fn execute(
    state: &AppState,
    input: &StartTurnInput,
    call: &crate::runtime::agent_tools::AgentToolCall,
) -> Result<Value, String> {
    validate(&call.name, &call.arguments)?;
    if state
        .shutdown_started
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err("app_stopping".into());
    }
    let canonical: Value =
        serde_json::from_str(&call.arguments).map_err(|_| "invalid_arguments")?;
    let digest = format!("{:x}", Sha256::digest(format!("{}:{canonical}", call.name)));
    // Replays must not probe or create a second process.
    let replay = state.sqlite_readers.read(|c| {
        let source = repo::source(c, &input.conversation_id, &input.run_id)?;
        if let Some(value) = repo::cached(c, &source, &call.id, &digest)? {
            return Ok(Some(value));
        }
        if call.name == "coding_start" {
            let prior = queries::prior_job_for_source(c, &source)?;
            if let Some((job, prior_digest)) = prior {
                if digest != prior_digest {
                    return Err("idempotency_conflict".into());
                }
                let mut value = repo::inspect(c, &input.conversation_id, &job, 0, 1)?;
                value["accepted"] = json!(true);
                return Ok(Some(value));
            }
        }
        Ok(None)
    })?;
    if let Some(value) = replay {
        return Ok(value);
    }
    if matches!(call.name.as_str(), "coding_start" | "coding_continue") {
        let settings = state.sqlite_readers.read(repo::settings)?;
        if !settings.enabled {
            return Err("coding_disabled".into());
        }
        state.sqlite_readers.read(|c| {
            if queries::active_run_exists(c)? {
                return Err("busy".into());
            }
            if call.name == "coding_start" {
                let valid = queries::workspace_belongs_to_conversation(
                    c,
                    canonical["workspaceId"].as_str(),
                    &input.conversation_id,
                )?;
                if !valid {
                    return Err("workspace_required".into());
                }
            } else {
                let job = canonical["jobId"].as_str().ok_or("invalid_arguments")?;
                repo::authorize(c, job, &input.conversation_id)?;
                repo::revision(
                    c,
                    job,
                    canonical["expectedRevision"]
                        .as_u64()
                        .ok_or("invalid_arguments")?,
                )?;
            }
            Ok(())
        })?;
        let implementation =
            if call.name == "coding_continue" {
                let job = canonical["jobId"].as_str().ok_or("invalid_arguments")?;
                state.sqlite_readers.read(|connection| {
                    let saved: String = connection.query_row(
                    "SELECT settings_json FROM coding_jobs WHERE id=?1 AND conversation_id=?2",
                    params![job, input.conversation_id],
                    |row| row.get(0),
                ).map_err(database_error)?;
                    serde_json::from_str::<CodingSettings>(&saved)
                        .map_err(|_| "coding_settings_invalid".into())
                })?
            } else {
                settings
            };
        probe_implementation(&implementation, &state.data_directory)?;
    }
    let mut launch = None;
    let result = state.sqlite_writer.write(|c| {
        if state
            .shutdown_started
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err("app_stopping".into());
        }
        let tx = c.transaction().map_err(database_error)?;
        crate::memory::personal_state::generation::allow_dispatch(&tx, &input.run_id)?;
        let source = repo::source(&tx, &input.conversation_id, &input.run_id)?;
        if let Some(value) = repo::cached(&tx, &source, &call.id, &digest)? {
            return Ok(value);
        }
        let calls = queries::call_count(&tx, &source)?;
        if calls >= 8 {
            return Err("coding_call_budget_exhausted".into());
        }
        let value = transactions::ToolTransaction {
            state,
            tx: &tx,
            input,
            call,
            canonical: &canonical,
            digest: &digest,
            source: &source,
        }
        .execute(&mut launch)?;
        queries::record_call(&tx, &source, &call.id, &digest, &value)?;
        tx.commit().map_err(database_error)?;
        Ok(value)
    })?;
    if let Some(run) = launch {
        let writer = Arc::clone(&state.sqlite_writer);
        std::thread::spawn(move || crate::runtime::pi::runner::run(writer, run));
    }
    Ok(result)
}

/// Starts the fixed, host-selected work for a persisted delegated task.
///
/// This deliberately does not manufacture a `StartTurnInput`: the task id is
/// the durable non-conversational origin and is recorded in
/// `coding_origin_bindings`.  `source_id` remains a per-run ledger key only.
pub fn execute_delegated(
    state: &AppState,
    conversation_id: &str,
    task_id: &str,
    workspace_id: &str,
    request: &str,
) -> Result<Value, String> {
    let settings = state.sqlite_readers.read(repo::settings)?;
    if !settings.enabled {
        return Err("coding_disabled".into());
    }
    probe_implementation(&settings, &state.data_directory)?;
    let mut launch = None;
    let value = state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        let (value, run) = commit_delegated_job(
            state,
            &transaction,
            conversation_id,
            task_id,
            workspace_id,
            request,
        )?;
        launch = run;
        transaction.commit().map_err(database_error)?;
        Ok(value)
    })?;
    if let Some(run) = launch {
        spawn_run(state, run);
    }
    Ok(value)
}

pub fn commit_delegated_job(
    state: &AppState,
    connection: &rusqlite::Connection,
    conversation_id: &str,
    task_id: &str,
    workspace_id: &str,
    request: &str,
) -> Result<(Value, Option<String>), String> {
    if state
        .shutdown_started
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err("app_stopping".into());
    }
    let settings = repo::settings(connection)?;
    if !settings.enabled {
        return Err("coding_disabled".into());
    }
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("delegated:{task_id}:{request}"))
    );
    let source = format!("delegated-{task_id}");
    let valid: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM steward_tasks t
              JOIN steward_delegations d ON d.id=t.delegation_id
              JOIN steward_goals g ON g.id=d.goal_id
              WHERE t.id=?1 AND t.conversation_id=?2 AND d.workspace_id=?3
                AND t.loop_state IN ('queued','dispatching') AND d.status='active' AND d.superseded_by IS NULL
                AND g.status='active' AND g.superseded_by IS NULL)",
            params![task_id, conversation_id, workspace_id],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if !valid {
        return Err("delegated_task_unavailable".into());
    }
    if queries::active_run_exists(connection)? {
        return Err("busy".into());
    }
    if let Some(job) = connection
        .query_row(
            "SELECT job_id FROM coding_origin_bindings WHERE origin_kind='delegated_event' AND origin_id=?1",
            [task_id],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?
    {
        let mut value = repo::inspect(connection, conversation_id, &job, 0, 1)?;
        value["accepted"] = json!(true);
        return Ok((value, None));
    }
    crate::steward::faults::maybe("coding_commit_before")?;
    let path = queries::workspace_path(connection, workspace_id, conversation_id)?;
    let actual = std::fs::canonicalize(&path).map_err(|_| "workspace_missing")?;
    if actual.to_str() != Some(path.as_str()) || !actual.join(".git").exists() {
        return Err("workspace_invalid".into());
    }
    let job = new_id("coding");
    let run = new_id("coding_run");
    let directory = state.data_directory.join("coding-sessions");
    std::fs::create_dir_all(&directory).map_err(|_| "session_storage_unavailable")?;
    let session = directory.join(format!("{job}.jsonl"));
    let settings_json = serde_json::to_string(&settings).map_err(|_| "coding_settings_invalid")?;
    queries::insert_job(
        connection,
        queries::NewJob {
            id: &job,
            conversation: conversation_id,
            source: &source,
            workspace: workspace_id,
            workspace_path: &path,
            settings_json: &settings_json,
            session_path: &session.to_string_lossy(),
            run: &run,
        },
    )?;
    connection
        .execute(
            "INSERT INTO coding_origin_bindings(id,job_id,origin_kind,origin_id,operation_digest,created_at)
             VALUES(?1,?2,'delegated_event',?3,?4,?5)",
            params![new_id("origin"), job, task_id, digest, now_iso()],
        )
        .map_err(database_error)?;
    let workspace_scope =
        crate::runtime::context::scope::register(connection, "resource", workspace_id)?;
    let job_scope = crate::runtime::context::scope::register(connection, "task", &job)?;
    crate::runtime::context::scope::link(connection, &workspace_scope, &job_scope)?;
    queries::insert_run(connection, &job, &run, &source, task_id, request, &digest)?;
    crate::steward::faults::maybe("local_receipt_before_spawn")?;
    Ok((
        json!({"jobId":job,"runId":run,"revision":1,"state":"queued","accepted":true}),
        Some(run),
    ))
}

pub fn spawn_run(state: &AppState, run: String) {
    let writer = Arc::clone(&state.sqlite_writer);
    std::thread::spawn(move || crate::runtime::pi::runner::run(writer, run));
}

fn probe_implementation(
    settings: &CodingSettings,
    directory: &std::path::Path,
) -> Result<(), String> {
    if !super::contracts::valid_implementation(settings) {
        return Err("coding_configuration_invalid".into());
    }
    if settings.implementation_method == "codex-sdk" {
        let mut child = crate::runtime::codex_cli::spawn_codex_app_server()?;
        child.kill().map_err(|_| "codex_probe_failed")?;
        child.wait().map_err(|_| "codex_probe_failed")?;
        Ok(())
    } else {
        crate::runtime::pi::process::probe(settings, directory)
    }
}

pub fn cancel(
    c: &rusqlite::Connection,
    conversation: &str,
    job: &str,
    revision: u64,
    reason: &str,
) -> Result<Value, String> {
    repo::authorize(c, job, conversation)?;
    let (status, run) = repo::revision(c, job, revision)?;
    if matches!(status.as_str(), "settled" | "failed" | "interrupted") {
        return Ok(json!({"jobId":job,"runId":run,"revision":revision,"state":status}));
    }
    if status == "outcome_unknown" {
        return Err("process_ownership_unresolved".into());
    }
    if status == "cancel_requested" {
        return Ok(
            json!({"jobId":job,"runId":run,"revision":revision,"state":status,"accepted":true}),
        );
    }
    queries::request_stop(c, job, &run, reason)?;
    repo::event(c, job, &run, "cancel_requested", json!({}))?;
    Ok(
        json!({"jobId":job,"runId":run,"revision":revision+1,"state":"cancel_requested","accepted":true}),
    )
}
pub fn register(state: &AppState, conversation: &str, path: &str) -> Result<Value, String> {
    let canonical = std::fs::canonicalize(PathBuf::from(path)).map_err(|_| "workspace_missing")?;
    if !canonical.is_dir() || !canonical.join(".git").exists() {
        return Err("workspace_not_git".into());
    }
    let git = std::process::Command::new("git")
        .arg("-C")
        .arg(&canonical)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|_| "workspace_not_git")?;
    if !git.status.success()
        || std::fs::canonicalize(String::from_utf8_lossy(&git.stdout).trim())
            .ok()
            .as_ref()
            != Some(&canonical)
    {
        return Err("workspace_not_git".into());
    }
    state.sqlite_writer.write(|c| {
        let id = workspace::replace(c, conversation, &canonical.to_string_lossy())?;
        Ok(json!({"workspaceId":id,"path":canonical.to_string_lossy()}))
    })
}
