use super::contracts::GoalProposal;
use super::repository as repo;
use crate::{validate_identifier, AppState};
use serde_json::Value;

#[tauri::command]
pub(crate) fn register_steward_goal(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    workspace_id: String,
    success_condition: String,
    summary: String,
    verifier: String,
    operations: String,
    budget_runs: u8,
    budget_ms: u64,
    notify: String,
    start_mode: Option<String>,
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    validate_identifier(&workspace_id, "workspace id")?;
    let start = start_mode.as_deref() == Some("start");
    let value = state.sqlite_writer.transact(|connection| {
        let value = repo::register_with_options(
            connection,
            &conversation_id,
            &workspace_id,
            &success_condition,
            &summary,
            &verifier,
            &operations,
            budget_runs,
            budget_ms,
            &notify,
        )?;
        if start {
            let goal_id = value["goalId"].as_str().ok_or("steward_register_invalid")?;
            let receipt = crate::new_id("ui_receipt");
            super::authority::persist_binding(
                connection,
                "goal",
                goal_id,
                &super::authority::SourceBinding {
                    kind: super::authority::SourceKind::UiReceipt,
                    id: receipt.clone(),
                    version: "1".into(),
                    quote_start: None,
                    quote_end: None,
                },
                None,
                None,
                Some(1),
                Some(1),
            )?;
            if let Some(work) = repo::active_goal_work(connection, &conversation_id, goal_id)? {
                let _ =
                    repo::queue_task(connection, &work, &conversation_id, &receipt, "admission")?;
            }
        }
        Ok(value)
    })?;
    if start {
        super::dispatch::wake(&state);
    }
    Ok(value)
}

#[tauri::command]
pub(crate) fn list_steward_goals(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    state
        .sqlite_readers
        .read(|connection| super::views::list_goals(connection, &conversation_id))
}

#[tauri::command]
pub(crate) fn work_confirm(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    proposal_id: String,
    expected_revision: i64,
    display_digest: String,
    start: bool,
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    validate_identifier(&proposal_id, "proposal id")?;
    let result = state.sqlite_writer.transact(|connection| {
        let result = super::intake::confirm(
            connection,
            &conversation_id,
            &proposal_id,
            expected_revision,
            &display_digest,
            start,
        )?;
        Ok(super::admission::json_result(result))
    })?;
    if start {
        super::dispatch::wake(&state);
    }
    Ok(result)
}

#[tauri::command]
pub(crate) fn work_resolve(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    task_id: String,
    expected_revision: i64,
    decision: String,
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    validate_identifier(&task_id, "task id")?;
    if !matches!(decision.as_str(), "approve" | "reject" | "reinspect") {
        return Err("work_resolve_invalid".into());
    }
    state.sqlite_writer.transact(|connection| {
        let revision: i64 = connection
            .query_row(
                "SELECT revision FROM steward_tasks WHERE id=?1 AND conversation_id=?2",
                rusqlite::params![&task_id, &conversation_id],
                |row| row.get(0),
            )
            .map_err(|_| "task_unavailable".to_string())?;
        if revision != expected_revision {
            return Err("stale_resolve".into());
        }
        let state_now: String = connection
            .query_row(
                "SELECT loop_state FROM steward_tasks WHERE id=?1",
                [&task_id],
                |row| row.get(0),
            )
            .map_err(|_| "task_unavailable".to_string())?;
        if state_now == "outcome_unknown" && decision != "reinspect" {
            return Err("unknown_requires_inspect".into());
        }
        let verifier: String = connection
            .query_row(
                "SELECT g.verifier FROM steward_tasks t
                 JOIN steward_delegations d ON d.id=t.delegation_id
                 JOIN steward_goals g ON g.id=d.goal_id WHERE t.id=?1",
                [&task_id],
                |row| row.get(0),
            )
            .map_err(|_| "task_unavailable".to_string())?;
        let next = next_resolve_state(&decision, &state_now, &verifier)?;
        repo::set_loop_state(connection, &task_id, next, None, None)?;
        Ok(serde_json::json!({"taskId": task_id, "loopState": next}))
    })
}

#[tauri::command]
pub(crate) fn register_steward_recipe(
    state: tauri::State<'_, AppState>,
    recipe: Value,
) -> Result<Value, String> {
    let input: super::recipes::RecipeInput =
        serde_json::from_value(recipe).map_err(|_| "recipe_invalid")?;
    state
        .sqlite_writer
        .transact(|connection| super::recipes::register(connection, &input))
}

#[tauri::command]
pub(crate) fn withdraw_steward_delegation(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    state.sqlite_writer.write(|connection| {
        let jobs = repo::active_job_ids(connection, &conversation_id)?;
        let value = repo::withdraw(connection, &conversation_id)?;
        for (job_id, revision) in jobs {
            let _ = crate::coding::service::cancel(
                connection,
                &conversation_id,
                &job_id,
                revision,
                "steward withdrawn",
            );
        }
        Ok(value)
    })
}

#[tauri::command]
pub(crate) fn list_steward_tasks(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    let result = state
        .sqlite_readers
        .read(|connection| repo::list(connection, &conversation_id))?;
    Ok(result)
}

/// Common work API.  It is also the target for the model-facing tool adapter;
/// keeping it here makes UI and tool proposals take the identical host checks.
#[tauri::command]
pub(crate) fn work_propose(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    proposal: Value,
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    let proposal: GoalProposal =
        serde_json::from_value(proposal).map_err(|_| "work_proposal_invalid")?;
    state
        .sqlite_writer
        .transact(|connection| repo::propose(connection, &conversation_id, &proposal))
}

#[tauri::command]
pub(crate) fn work_status(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Value, String> {
    list_steward_tasks(state, conversation_id)
}

#[tauri::command]
pub(crate) fn work_withdraw(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    goal_id: Option<String>,
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    match goal_id {
        Some(goal_id) => {
            validate_identifier(&goal_id, "goal id")?;
            state.sqlite_writer.write(|connection| {
                let jobs = repo::active_goal_job_ids(connection, &conversation_id, &goal_id)?;
                let value = repo::withdraw_goal(connection, &conversation_id, &goal_id)?;
                for (job_id, revision) in jobs {
                    let _ = crate::coding::service::cancel(
                        connection,
                        &conversation_id,
                        &job_id,
                        revision,
                        "steward withdrawn",
                    );
                }
                Ok(value)
            })
        }
        None => withdraw_steward_delegation(state, conversation_id),
    }
}

/// A deliberately narrow amendment.  Broadening operations or budgets needs a
/// new proposal because it is a new authority grant.
#[tauri::command]
pub(crate) fn work_amend(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    goal_id: String,
    notify: String,
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    validate_identifier(&goal_id, "goal id")?;
    if !matches!(notify.as_str(), "both" | "silent" | "speak") {
        return Err("work_amend_invalid".into());
    }
    state.sqlite_writer.write(|connection| {
        let changed = connection.execute(
            "UPDATE steward_delegations SET notify=?1,revision=revision+1 WHERE goal_id=?2 AND conversation_id=?3 AND status='active' AND superseded_by IS NULL",
            rusqlite::params![&notify, &goal_id, &conversation_id],
        ).map_err(crate::database_error)?;
        if changed == 0 { return Err("goal_unavailable".into()); }
        let revision: i64 = connection
            .query_row(
                "SELECT revision FROM steward_delegations WHERE goal_id=?1 AND conversation_id=?2 AND status='active' AND superseded_by IS NULL",
                rusqlite::params![&goal_id, &conversation_id],
                |row| row.get(0),
            )
            .map_err(crate::database_error)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        crate::adaptive_improvement::set_override(
            connection,
            crate::adaptive_improvement::Domain::Notification,
            &goal_id,
            &notify,
            &format!("steward-amend:{goal_id}"),
            revision,
            None,
            now,
        )?;
        Ok(serde_json::json!({"goalId": goal_id, "status":"active", "revisioned":true}))
    })
}

pub(crate) fn next_resolve_state(
    decision: &str,
    loop_state: &str,
    verifier: &str,
) -> Result<&'static str, String> {
    match decision {
        "approve" if loop_state == "awaiting_user" && verifier == "user_confirmation_required" => {
            Ok("done")
        }
        "approve" if loop_state == "awaiting_user" => Err("evidence_required".into()),
        "reject" => Ok("cancelled"),
        "reinspect" => Ok("queued"),
        _ => Err("work_resolve_invalid".into()),
    }
}
