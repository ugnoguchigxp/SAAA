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
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    validate_identifier(&workspace_id, "workspace id")?;
    state.sqlite_writer.write(|connection| {
        repo::register(
            connection,
            &conversation_id,
            &workspace_id,
            &success_condition,
        )
    })
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
    let result = state.sqlite_writer.write(|connection| {
        repo::sync_from_coding(connection, &conversation_id)?;
        repo::list(connection, &conversation_id)
    })?;
    // A UI refresh is a delivery wake-up, not a user turn.  The report module
    // still respects the Situation hold before inserting a chat message.
    super::report::flush_held_reports(&state, &conversation_id)?;
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
        .write(|connection| repo::propose(connection, &conversation_id, &proposal))
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
