use super::{
    contracts::{
        MountArtifactPreviewInput, PrepareArtifactPreviewInput, ReleaseArtifactPreviewInput,
    },
    host, service,
};
use crate::AppState;

#[tauri::command]
pub(crate) fn mount_source_website(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<String, String> {
    super::source_web::mount(&app, &state, &conversation_id, &url, x, y, width, height)
}

#[tauri::command]
pub(crate) fn open_source_website_in_browser(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
    url: String,
) -> Result<(), String> {
    super::source_web::open_in_browser(&state, &conversation_id, &url)
}

#[tauri::command]
pub(crate) fn scroll_source_website(app: tauri::AppHandle, label: String) -> Result<(), String> {
    super::source_web::scroll(&app, &label)
}

#[tauri::command]
pub(crate) fn report_artifact_webview_session(
    conversation_id: String,
    generation: u64,
    labels: Vec<String>,
    selected: Option<usize>,
    scrollable: bool,
    mounted: bool,
) {
    super::webview_ops::report_session(super::webview_ops::WebviewSession {
        conversation_id,
        generation,
        labels,
        selected,
        scrollable,
        mounted,
        other_artifacts: Vec::new(),
    });
}

#[tauri::command]
pub(crate) fn poll_artifact_webview_request() -> Option<super::webview_ops::WebviewRequest> {
    super::webview_ops::poll_request()
}

#[tauri::command]
pub(crate) fn complete_artifact_webview_request(request_id: String, applied: bool) {
    super::webview_ops::complete_request(&request_id, applied);
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
