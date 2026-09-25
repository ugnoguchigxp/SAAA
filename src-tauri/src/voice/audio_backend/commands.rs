use super::{global, AudioBackendStatus, VoiceProcessingConfig};
use crate::{persistence::load_voice_settings, AppState};
use std::sync::Arc;
use tauri::{ipc::Channel, State};

fn config_from_state(state: &AppState) -> Result<VoiceProcessingConfig, String> {
    state
        .sqlite_readers
        .read(load_voice_settings)
        .map(|settings| {
            VoiceProcessingConfig::from_settings(
                settings.aec_enabled,
                &settings.other_audio_ducking,
                settings.vpio_on_bluetooth,
            )
        })
        .map_err(|_| "Could not load VoiceProcessing settings".to_string())
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
    let config = config_from_state(&state)?;
    let on_frame = std::sync::Mutex::new(on_frame);
    let sink = Arc::new(move |frame: Vec<f32>| {
        on_frame
            .lock()
            .map(|channel| channel.send(frame).is_ok())
            .unwrap_or(false)
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
