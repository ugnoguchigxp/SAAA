use super::repository as repo;
use crate::{validate_identifier, AppState};
use serde_json::Value;

#[tauri::command]
pub(crate) fn reorder_steward_queue(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    task_ids: Vec<String>,
) -> Result<Value, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    for task_id in &task_ids {
        validate_identifier(task_id, "task id")?;
    }
    state.sqlite_writer.transact(|connection| {
        repo::reorder_queue(connection, &conversation_id, &task_ids)?;
        Ok(serde_json::json!({"status":"reordered","taskIds":task_ids}))
    })
}

