//! Desktop command adapter. Ordering and persistence live in `saaa_media::MediaService`.
use super::*;
use saaa_media::RunHandle;

#[tauri::command]
pub(crate) async fn generate_media(
    state: tauri::State<'_, AppState>,
    input: GenerateInput,
    on_progress: tauri::ipc::Channel<MediaProgress>,
) -> Result<GenerateOutput, String> {
    let handle = state
        .media
        .submit(input)
        .await
        .map_err(|error| error.message)?;
    forward(handle, &on_progress).await
}

pub(super) async fn forward(
    mut handle: RunHandle,
    on_progress: &tauri::ipc::Channel<MediaProgress>,
) -> Result<GenerateOutput, String> {
    while let Some(progress) = handle.next_progress().await {
        if handle.terminal().is_some() {
            break;
        }
        let _ = on_progress.send(progress);
    }
    handle.wait_terminal().await.into_command()
}
