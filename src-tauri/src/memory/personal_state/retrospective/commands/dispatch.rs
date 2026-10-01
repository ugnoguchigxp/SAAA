use super::*;
/// Domain commands follow the existing feature-owned IPC registration pattern.
pub(crate) fn with_handler<R: tauri::Runtime>(
    fallback: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    let review: fn(tauri::ipc::Invoke<R>) -> bool =
        tauri::generate_handler![set_world_review_mode, world_review_candidates];
    move |invoke: tauri::ipc::Invoke<R>| match invoke.message.command() {
        "set_world_review_mode" | "world_review_candidates" => review(invoke),
        _ => fallback(invoke),
    }
}
