use std::sync::Arc;

use crate::{
    voice, AppState, NetworkAsrResolution, ResolveNetworkAsrInput, RunCancellation, SpeakTextInput,
    TtsCapabilities,
};

#[tauri::command]
pub(crate) fn get_voice_profile_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<voice::profile::VoiceProfileSnapshot, String> {
    state
        .voice_profile
        .read_with_snapshot(&state.sqlite_readers, |_connection, snapshot| Ok(snapshot))
}

#[tauri::command]
pub(crate) fn stage_audio_upload(
    state: tauri::State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<String, String> {
    state.audio_uploads.stage(request)
}

#[tauri::command]
pub(crate) fn save_voice_enrollment_sample(
    state: tauri::State<'_, AppState>,
    mut input: voice::profile::SaveVoiceEnrollmentSampleInput,
) -> Result<voice::profile::VoiceProfileSnapshot, String> {
    if state.meeting.blocks_tts() {
        return Err(
            "Voice enrollment is unavailable while a meeting is active or paused".to_string(),
        );
    }
    if state
        .tts_process
        .lock()
        .map(|value| value.is_some())
        .unwrap_or(true)
    {
        return Err("Stop speech playback before recording an enrollment sample".to_string());
    }
    if state.streaming_tts.is_active() {
        return Err("Stop speech playback before recording an enrollment sample".to_string());
    }
    input.samples = state
        .audio_uploads
        .consume(&input.audio_upload_id, "voice-enrollment")?;
    state.voice_profile.save_sample(&state.sqlite_writer, input)
}

#[tauri::command]
pub(crate) fn set_target_speaker_filter_enabled(
    state: tauri::State<'_, AppState>,
    input: voice::profile::SetTargetSpeakerFilterInput,
) -> Result<voice::profile::VoiceProfileSnapshot, String> {
    state
        .voice_profile
        .set_filter_enabled(&state.sqlite_writer, input.enabled)
}

#[tauri::command]
pub(crate) fn delete_voice_enrollment_sample(
    state: tauri::State<'_, AppState>,
    sample_id: String,
) -> Result<voice::profile::VoiceProfileSnapshot, String> {
    state
        .voice_profile
        .delete_sample(&state.sqlite_writer, &sample_id)
}

#[tauri::command]
pub(crate) fn delete_voice_profile(
    state: tauri::State<'_, AppState>,
) -> Result<voice::profile::VoiceProfileSnapshot, String> {
    state.voice_profile.delete_profile(&state.sqlite_writer)
}

#[tauri::command]
pub(crate) fn read_voice_enrollment_sample(
    state: tauri::State<'_, AppState>,
    sample_id: String,
) -> Result<tauri::ipc::Response, String> {
    state
        .voice_profile
        .read_sample(&state.sqlite_readers, &sample_id)
        .map(tauri::ipc::Response::new)
}

#[tauri::command]
pub(crate) async fn resolve_network_asr(
    state: tauri::State<'_, AppState>,
    input: ResolveNetworkAsrInput,
) -> Result<NetworkAsrResolution, String> {
    state
        .network_asr
        .refresh(&input.host, Arc::new(RunCancellation::default()))
        .await
}

#[tauri::command]
pub(crate) async fn speak_text(
    state: tauri::State<'_, AppState>,
    input: SpeakTextInput,
) -> Result<(), String> {
    voice::session::speak_text(&state, input).await
}

#[tauri::command]
pub(crate) fn list_tts_capabilities() -> TtsCapabilities {
    voice::system_tts::capabilities()
}

#[tauri::command]
pub(crate) fn stop_tts(state: tauri::State<'_, AppState>, run_id: String) -> Result<(), String> {
    voice::session::stop_tts(&state, run_id)
}
