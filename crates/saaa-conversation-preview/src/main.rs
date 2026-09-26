use saaa_conversation_preview::host;
use tauri::{AppHandle, Manager, RunEvent, State};

#[tauri::command]
async fn open_preview(
    app: AppHandle,
    state: State<'_, host::PreviewApp>,
) -> Result<host::Snapshot, String> {
    state.open(app).await
}

#[tauri::command]
async fn submit_text(
    state: State<'_, host::PreviewApp>,
    input_id: String,
    text: String,
) -> Result<host::SubmitReceipt, String> {
    state.current().await?.submit(input_id, text)
}

#[tauri::command]
async fn stop_response(state: State<'_, host::PreviewApp>, scope: String) -> Result<(), String> {
    state.current().await?.stop(&scope).await
}

#[tauri::command]
async fn get_snapshot(state: State<'_, host::PreviewApp>) -> Result<host::Snapshot, String> {
    state.current().await?.snapshot()
}

#[tauri::command]
async fn close_preview(state: State<'_, host::PreviewApp>) -> Result<(), String> {
    state.close().await
}

fn main() {
    tauri::Builder::default()
        .manage(host::PreviewApp::default())
        .invoke_handler(tauri::generate_handler![
            open_preview,
            submit_text,
            stop_response,
            get_snapshot,
            close_preview
        ])
        .build(tauri::generate_context!())
        .expect("conversation preview failed to start")
        .run(|app, event| {
            if matches!(event, RunEvent::Exit) {
                let _ = tauri::async_runtime::block_on(app.state::<host::PreviewApp>().close());
            }
        });
}
