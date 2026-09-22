use super::{
    contracts::{
        MountArtifactPreviewInput, PrepareArtifactPreviewInput, ReleaseArtifactPreviewInput,
    },
    host, service,
};
use crate::AppState;

#[tauri::command]
pub(crate) fn prepare_artifact_preview(
    state: tauri::State<'_, AppState>,
    input: PrepareArtifactPreviewInput,
) -> Result<super::contracts::ArtifactPreviewDescriptor, String> {
    service::prepare(&state.artifact_preview, &input)
}

#[tauri::command]
pub(crate) fn release_artifact_preview(
    state: tauri::State<'_, AppState>,
    input: ReleaseArtifactPreviewInput,
) -> Result<(), String> {
    service::release(&state.artifact_preview, &input.preview_token);
    Ok(())
}

#[tauri::command]
pub(crate) fn mount_artifact_preview(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    input: MountArtifactPreviewInput,
) -> Result<(), String> {
    host::mount(&app, &state.artifact_preview, &input)
}
