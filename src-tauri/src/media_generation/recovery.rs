//! Recovery only polls a stored remote ID. It never creates another prediction.
use super::*;
use saaa_media::HistoryQuery;

#[tauri::command]
pub(crate) fn list_media_generations(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Value>, String> {
    state
        .media
        .history(&HistoryQuery::Latest { limit: 20 })
        .map_err(|error| error.message)
}

#[tauri::command]
pub(crate) async fn reconcile_media_generation(
    state: tauri::State<'_, AppState>,
    run_id: String,
    on_progress: tauri::ipc::Channel<MediaProgress>,
) -> Result<Value, String> {
    let handle = state
        .media
        .reconcile(&run_id)
        .await
        .map_err(|error| error.message)?;
    let output = generation::forward(handle, &on_progress).await?;
    serde_json::to_value(output).map_err(|error| error.to_string())
}
