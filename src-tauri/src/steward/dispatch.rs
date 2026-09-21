//! Dispatch linearization: authority, slot, job, binding, and local receipt commit together.
use super::repository as repo;
use crate::situation::contracts::ForegroundCategory;
use crate::AppState;
use rusqlite::params;

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
    conversation_id: String,
    task_id: String,
    launch: Option<String>,
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
    if repo::budget_exceeded(connection, &work)? {
        repo::set_loop_state(connection, &task_id, "awaiting_user", None, Some("budget"))?;
        return Ok(None);
    }
    if !repo::workspace_registered(connection, &conversation_id, &work.workspace_id)? {
        return Ok(None);
    }
    if super::queue::idempotent_accepted(connection, &task_id)?.is_some() {
        return Ok(None);
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
    if !repo::delegated_profile_available(state)? {
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
    match crate::coding::service::commit_delegated_job(
        state,
        connection,
        &conversation_id,
        &task_id,
        &work.workspace_id,
        request,
    ) {
        Ok((value, launch)) => {
            let job = value["jobId"].as_str();
            repo::set_loop_state(connection, &task_id, "running", job, None)?;
            repo::settle_dispatch(connection, &task_id, Some(&value), false)?;
            Ok(Some(Prepared {
                conversation_id,
                task_id,
                launch,
            }))
        }
        Err(error) if matches!(error.as_str(), "busy" | "coding_disabled") => {
            super::queue::park_pending(connection, &task_id, &error)?;
            repo::set_loop_state(connection, &task_id, "queued", None, Some(&error))?;
            Ok(None)
        }
        Err(error) => {
            repo::set_loop_state(connection, &task_id, "failed", None, Some(&error))?;
            repo::settle_dispatch(connection, &task_id, None, true)?;
            Err(error)
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
    let _ = prepared.conversation_id;
    let _ = prepared.task_id;
    Ok(())
}

pub(crate) fn wake(state: &AppState) {
    state.steward_wake.signal();
}
