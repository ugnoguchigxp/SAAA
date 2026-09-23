use crate::runtime::image_input::{self, ImageError, PreparedImage};
use crate::AppState;

fn map_error(error: ImageError) -> String {
    error.to_string()
}

#[tauri::command]
pub(crate) async fn prepare_composer_image(
    state: tauri::State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<PreparedImage, String> {
    let tauri::ipc::InvokeBody::Raw(png) = request.body() else {
        return Err("image upload must use binary IPC".into());
    };
    if png.len() > image_input::MAX_INPUT_BYTES {
        return Err("image input is too large".into());
    }
    let png = png.to_vec();
    let data_directory = state.data_directory.clone();
    tokio::task::spawn_blocking(move || image_input::prepare(&data_directory, &png))
        .await
        .map_err(|_| "image processing could not finish".to_string())?
        .map_err(map_error)
}

#[tauri::command]
pub(crate) fn discard_composer_image(
    state: tauri::State<'_, AppState>,
    image_id: String,
) -> Result<(), String> {
    image_input::discard(&state.data_directory, &image_id).map_err(map_error)
}

#[tauri::command]
pub(crate) fn claim_turn_image(
    state: tauri::State<'_, AppState>,
    image_id: String,
    run_id: String,
) -> Result<(), String> {
    image_input::claim(&state.data_directory, &image_id, &run_id).map_err(map_error)
}

#[tauri::command]
pub(crate) fn confirm_image_input(
    state: tauri::State<'_, AppState>,
    endpoint: String,
    model: String,
    format: String,
) -> Result<(), String> {
    if endpoint.is_empty()
        || model.is_empty()
        || endpoint.len() > 2_048
        || model.len() > 200
        || endpoint.chars().any(|character| character.is_control())
        || model.chars().any(|character| character.is_control())
    {
        return Err("image provider confirmation is invalid".into());
    }
    let format = match format.as_str() {
        "webp" => image_input::SendFormat::Webp,
        "png" => image_input::SendFormat::Png,
        "jpeg" => image_input::SendFormat::Jpeg,
        "unsupported" => image_input::SendFormat::Unsupported,
        _ => return Err("image format is not confirmed".into()),
    };
    image_input::store_capability(&state.data_directory, &endpoint, &model, format)
        .map_err(map_error)
}

#[tauri::command]
pub(crate) fn release_turn_image(
    state: tauri::State<'_, AppState>,
    run_id: String,
) -> Result<(), String> {
    image_input::release(&state.data_directory, &run_id).map_err(map_error)
}
