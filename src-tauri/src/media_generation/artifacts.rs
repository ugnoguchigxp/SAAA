//! Adopted artifacts only. Bytes come from the shared service, never from an arbitrary path.
use super::*;

#[tauri::command]
pub(crate) async fn read_generated_media(
    state: tauri::State<'_, AppState>,
    run_id: String,
    artifact_index: usize,
) -> Result<tauri::ipc::Response, String> {
    let artifact = state
        .media
        .artifact(&run_id, artifact_index)
        .await
        .map_err(|error| error.message)?;
    Ok(tauri::ipc::Response::new(artifact.bytes))
}
