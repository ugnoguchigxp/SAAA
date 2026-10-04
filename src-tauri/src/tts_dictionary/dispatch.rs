use super::*;
pub(crate) fn with_handler(
    fallback: impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    let handler: fn(tauri::ipc::Invoke) -> bool = tauri::generate_handler![
        list_tts_dictionary,
        search_tts_dictionary_presets,
        lookup_tts_dictionary_entry,
        save_tts_dictionary_entry,
        delete_tts_dictionary_entry,
        preview_tts_dictionary
    ];
    move |invoke| match invoke.message.command() {
        "list_tts_dictionary"
        | "search_tts_dictionary_presets"
        | "lookup_tts_dictionary_entry"
        | "save_tts_dictionary_entry"
        | "delete_tts_dictionary_entry"
        | "preview_tts_dictionary" => handler(invoke),
        _ => fallback(invoke),
    }
}
