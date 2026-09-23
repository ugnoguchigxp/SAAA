use super::{
    contracts::{
        MountArtifactPreviewInput, PrepareArtifactPreviewInput, ReleaseArtifactPreviewInput,
    },
    host, service,
};
use crate::AppState;

#[tauri::command]
pub(crate) fn read_source_artifact(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    url: String,
) -> Result<Option<super::source::SourceArtifact>, String> {
    let principal = crate::tool_selection::service::ensure_principal(&state.sqlite_writer)
        .map_err(|error| error.to_string())?;
    state.sqlite_readers.read(|connection| {
        let auth = crate::records::auth::Authorization {
            principal_id: principal,
            conversation_id,
            allowed_scope_keys: Vec::new(),
        };
        super::source::read_source(connection, &auth, &url)
    })
}

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
