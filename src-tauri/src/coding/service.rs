use super::{contracts::*, repository as repo};
use crate::{database_error, AppState, StartTurnInput};
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
        crate::runtime::pi::process::probe(&settings, &state.data_directory)?;
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
