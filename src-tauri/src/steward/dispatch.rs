//! Dispatch linearization: authority, slot, job, binding, and local receipt commit together.
use super::repository as repo;
use crate::situation::contracts::ForegroundCategory;
use crate::AppState;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

pub(crate) fn start_queued_for_conversation(
    state: &AppState,
    conversation_id: &str,
) -> Result<(), String> {
    if !gates_allow_new_work(state)? {
        return Ok(());
    }
    let prepared = state
        .sqlite_writer
        .transact(|connection| prepare_one(state, connection, Some(conversation_id)))?;
    finish(state, prepared)
}

pub(crate) fn start_next_global(state: &AppState) -> Result<(), String> {
    if !gates_allow_new_work(state)? {
        return Ok(());
    }
    let prepared = state
        .sqlite_writer
        .transact(|connection| prepare_one(state, connection, None))?;
    finish(state, prepared)
}

/// Schedule fires one already-queued task by durable ids. Job binding and the
/// local receipt stay in the same writer transaction as `commit_delegated_job`.
pub(crate) fn dispatch_scheduled(
    state: &AppState,
    task_id: &str,
    delegation_id: &str,
) -> Result<Value, String> {
    if !gates_allow_new_work(state)? {
        return Err("dispatch_gated".into());
    }
    let prepared = state
        .sqlite_writer
        .transact(|connection| prepare_named(state, connection, task_id, delegation_id))?;
    let receipt = prepared
        .as_ref()
        .and_then(|prepared| prepared.receipt.clone())
        .ok_or_else(|| "dispatch_unavailable".to_string())?;
    finish(state, prepared)?;
    Ok(receipt)
}

fn gates_allow_new_work(state: &AppState) -> Result<bool, String> {
    if !crate::memory::control_plane::memory_enabled() {
        return Ok(false);
    }
    if !repo::coding_enabled(state)? {
        return Ok(false);
    }
    if matches!(
        state.situation.foreground_category(),
        ForegroundCategory::Coding
    ) {
        return Ok(false);
    }
    Ok(true)
}

struct Prepared {
    launch: Option<String>,
    receipt: Option<Value>,
}

fn prepare_one(
    state: &AppState,
    connection: &rusqlite::Connection,
    conversation_id: Option<&str>,
) -> Result<Option<Prepared>, String> {
    crate::steward::faults::maybe("claim_before")?;
    let Some((work, task_id, conversation_id)) =
        super::queue::next_eligible(connection, conversation_id)?
    else {
        return Ok(None);
    };
    prepare_candidate(state, connection, work, task_id, conversation_id)
}

fn prepare_named(
    state: &AppState,
    connection: &rusqlite::Connection,
    task_id: &str,
    delegation_id: &str,
) -> Result<Option<Prepared>, String> {
    crate::steward::faults::maybe("claim_before")?;
    let row: Option<(repo::ActiveWork, String)> = connection
        .query_row(
            "SELECT g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,0,d.ops,g.verifier,t.conversation_id
             FROM steward_tasks t
             JOIN steward_delegations d ON d.id=t.delegation_id
             JOIN steward_goals g ON g.id=d.goal_id
             WHERE t.id=?1 AND d.id=?2 AND t.loop_state='queued'
               AND g.status='active' AND g.superseded_by IS NULL
               AND d.status='active' AND d.superseded_by IS NULL",
            params![task_id, delegation_id],
            |r| {
                Ok((
                    repo::ActiveWork {
                        goal_id: r.get(0)?,
                        goal_status: r.get(1)?,
                        delegation_id: r.get(2)?,
                        workspace_id: r.get(3)?,
                        budget_runs: r.get(4)?,
                        budget_ms: r.get(5)?,
                        superseded: r.get::<_, i64>(6)? != 0,
                        ops: r.get(7)?,
                        verifier: r.get(8)?,
                    },
                    r.get(9)?,
                ))
            },
        )
        .optional()
        .map_err(crate::database_error)?;
    let Some((work, conversation_id)) = row else {
        return Ok(None);
    };
    prepare_candidate(
        state,
        connection,
        work,
        task_id.to_string(),
        conversation_id,
    )
}

fn prepare_candidate(
    state: &AppState,
    connection: &rusqlite::Connection,
    work: repo::ActiveWork,
    task_id: String,
    conversation_id: String,
) -> Result<Option<Prepared>, String> {
    if repo::budget_exceeded(connection, &work)? {
        repo::set_loop_state(connection, &task_id, "awaiting_user", None, Some("budget"))?;
        return Ok(None);
    }
    if !repo::workspace_registered(connection, &conversation_id, &work.workspace_id)? {
        repo::set_loop_state(
            connection,
            &task_id,
            "awaiting_user",
            None,
            Some("workspace_required"),
        )?;
        super::queue::park_pending(connection, &task_id, "workspace_required")?;
        return Ok(None);
    }
    match super::queue::idempotent_accepted(connection, &task_id) {
        Ok(Some(receipt)) => {
            repo::set_loop_state(
                connection,
                &task_id,
                "running",
                receipt["jobId"].as_str(),
                None,
            )?;
            return Ok(None);
        }
        Err(error) if error == "outcome_unknown" => {
            repo::set_loop_state(
                connection,
                &task_id,
                "outcome_unknown",
                None,
                Some("outcome_unknown"),
            )?;
            return Ok(None);
        }
        Ok(None) => {}
        Err(error) => return Err(error),
    }
    if !repo::claim_dispatch(connection, &task_id)? {
        return Ok(None);
    }
    crate::steward::faults::maybe("revoke_before_dispatch")?;
    let still_valid: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id JOIN steward_goals g ON g.id=d.goal_id
             WHERE t.id=?1 AND g.status='active' AND d.status='active')",
            [&task_id],
            |row| row.get(0),
        )
        .map_err(crate::database_error)?;
    if !still_valid {
        repo::set_loop_state(connection, &task_id, "cancelled", None, Some("revoked"))?;
        connection
            .execute(
                "UPDATE steward_dispatch_intents SET state='failed',last_reason='revoked',updated_at=?2 WHERE task_id=?1",
                params![task_id, crate::now_iso()],
            )
            .map_err(crate::database_error)?;
        return Ok(None);
    }
    if source_now_forbids(connection, &conversation_id, &task_id, &work)? {
        repo::set_loop_state(
            connection,
            &task_id,
            "cancelled",
            None,
            Some("source_withdrawn"),
        )?;
        connection
            .execute(
                "UPDATE steward_dispatch_intents SET state='failed',last_reason='source_withdrawn',updated_at=?2 WHERE task_id=?1",
                params![task_id, crate::now_iso()],
            )
            .map_err(crate::database_error)?;
        return Ok(None);
    }
    if !repo::delegated_profile_available(connection)? {
        repo::set_loop_state(
            connection,
            &task_id,
            "awaiting_user",
            None,
            Some("delegated_profile_required"),
        )?;
        super::queue::park_pending(connection, &task_id, "profile")?;
        return Ok(None);
    }
    let request = super::queue::request_for_task(connection, &work, &task_id)?;
    let recipe = super::queue::recipe_for_task(connection, &work, &task_id)?;
    repo::persist_task_plan(connection, &task_id, recipe, &request, "host_selected", 1)?;
    match crate::coding::service::commit_delegated_job(
        state,
        connection,
        &conversation_id,
        &task_id,
        &work.workspace_id,
        &request,
    ) {
        Ok((value, launch)) => {
            if let Some(run_id) = launch.as_deref() {
                let remaining = super::budget::remaining_deadline_ms(connection, &work)?;
                super::budget::arm(run_id, remaining);
            }
            let job = value["jobId"].as_str();
            repo::set_loop_state(connection, &task_id, "running", job, None)?;
            repo::settle_dispatch(connection, &task_id, Some(&value), false)?;
            Ok(Some(Prepared {
                launch,
                receipt: Some(value),
            }))
        }
        Err(error) if matches!(error.as_str(), "busy" | "coding_disabled") => {
            super::queue::park_pending(connection, &task_id, &error)?;
            repo::set_loop_state(connection, &task_id, "queued", None, Some(&error))?;
            Ok(None)
        }
        Err(error)
            if matches!(
                error.as_str(),
                "workspace_invalid" | "workspace_missing" | "workspace_required"
            ) =>
        {
            repo::set_loop_state(connection, &task_id, "awaiting_user", None, Some(&error))?;
            super::queue::park_pending(connection, &task_id, &error)?;
            Ok(None)
        }
        Err(error) => {
            repo::set_loop_state(connection, &task_id, "failed", None, Some(&error))?;
            repo::settle_dispatch(connection, &task_id, None, true)?;
            Ok(None)
        }
    }
}

fn finish(state: &AppState, prepared: Option<Prepared>) -> Result<(), String> {
    let Some(prepared) = prepared else {
        return Ok(());
    };
    crate::steward::faults::maybe("spawn")?;
    if let Some(run) = prepared.launch {
        crate::coding::service::spawn_run(state, run);
    }
    Ok(())
}

pub(crate) fn wake(state: &AppState) {
    state.steward_wake.signal();
}

fn source_now_forbids(
    connection: &rusqlite::Connection,
    conversation_id: &str,
    task_id: &str,
    work: &repo::ActiveWork,
) -> Result<bool, String> {
    let (source_id, bound_version): (String, Option<String>) = connection
        .query_row(
            "SELECT t.source_id,
                    (SELECT b.source_version FROM steward_source_bindings b
                     WHERE b.subject_id=t.id OR b.subject_id=?2
                     ORDER BY b.rowid DESC LIMIT 1)
             FROM steward_tasks t WHERE t.id=?1",
            rusqlite::params![task_id, work.goal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(crate::database_error)?;
    let Ok(current) =
        super::authority::current_source_digest(connection, conversation_id, &source_id)
    else {
        return Ok(false);
    };
    if bound_version
        .as_deref()
        .is_some_and(|version| version != current)
    {
        return Ok(true);
    }
    let text: Result<String, _> = connection.query_row(
        "SELECT content FROM conversation_messages WHERE id=?1 AND conversation_id=?2",
        rusqlite::params![source_id, conversation_id],
        |row| row.get(0),
    );
    let Ok(text) = text else {
        return Ok(true);
    };
    let operations = match work.ops.as_str() {
        "read" => vec![super::contracts::Operation::Read],
        "test_run" => vec![super::contracts::Operation::TestRun],
        _ => vec![
            super::contracts::Operation::Read,
            super::contracts::Operation::TestRun,
        ],
    };
    let decision =
        super::request_intent::overall(&super::request_intent::classify_source(&text, &operations));
    Ok(decision == super::request_intent::IntentDecision::Denied)
}
