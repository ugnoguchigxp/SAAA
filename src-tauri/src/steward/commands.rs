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
    state.sqlite_writer.write(|connection| {
        repo::sync_from_coding(connection, &conversation_id)?;
        repo::list(connection, &conversation_id)
    })
}
