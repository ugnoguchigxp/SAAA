use super::{global, AudioBackendStatus, VoiceProcessingConfig};
use crate::{persistence::load_voice_settings, AppState};
use std::sync::Arc;
use tauri::{ipc::Channel, State};

fn config_from_state(state: &AppState) -> VoiceProcessingConfig {
    state
        .sqlite_readers
        .read(|connection| load_voice_settings(connection))
        .map(|settings| {
            VoiceProcessingConfig::from_settings(
                settings.aec_enabled,
                &settings.other_audio_ducking,
                settings.vpio_on_bluetooth,
            )
        })
        .unwrap_or_default()
}

#[tauri::command]
pub(crate) fn audio_backend_status() -> AudioBackendStatus {
    global().status()
}

#[tauri::command]
pub(crate) fn start_native_voice_capture(
    state: State<'_, AppState>,
    on_frame: Channel<Vec<f32>>,
) -> Result<AudioBackendStatus, String> {
    let config = config_from_state(&state);
    let on_frame = std::sync::Mutex::new(on_frame);
    let sink = Arc::new(move |frame: Vec<f32>| {
        if let Ok(channel) = on_frame.try_lock() {
            let _ = channel.send(frame);
        }
    });
    global().start_capture(config, sink)
}

#[tauri::command]
pub(crate) fn stop_native_voice_capture() -> Result<(), String> {
    global().stop_capture();
    Ok(())
}

#[tauri::command]
pub(crate) fn interrupt_native_voice_playback() -> Result<(), String> {
    global().interrupt_playback();
    Ok(())
}
