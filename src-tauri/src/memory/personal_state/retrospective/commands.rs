use crate::AppState;
use serde_json::Value;
#[tauri::command]
pub fn set_world_review_mode(
    state: tauri::State<'_, AppState>,
    mode: String,
) -> Result<Value, String> {
    super::super::worker::interrupt();
    state.sqlite_writer.transact(|c| {
        super::set_mode(c, &mode)?;
        super::super::jobs::refill_reviews(c)?;
        Ok(())
    })?;
    state
        .sqlite_writer
        .read_serialized(super::super::commands::snapshot)
}

#[tauri::command]
pub fn world_review_candidates(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    state
        .sqlite_writer
        .read_serialized(super::projection::candidates)
}

mod dispatch;
pub(crate) use dispatch::with_handler;
