use super::*;
pub(crate) fn with_handler<R: tauri::Runtime>(
    fallback: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    let handler: fn(tauri::ipc::Invoke<R>) -> bool = tauri::generate_handler![
        conversation_asr_transport,
        start_qwen_asr_session,
        append_qwen_asr_audio,
        stop_qwen_asr_session
    ];
    move |invoke| match invoke.message.command() {
        "conversation_asr_transport"
        | "start_qwen_asr_session"
        | "append_qwen_asr_audio"
        | "stop_qwen_asr_session" => handler(invoke),
        _ => fallback(invoke),
    }
}
