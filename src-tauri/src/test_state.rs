//! Test-only AppState construction. Kept in its own module so the provider helpers in
//! `test_support` stay small and the shared writer/capability-service variant is explicit.

use crate::{
    generated_capabilities::{publication::GeneratedToolsConfig, service::CapabilityService},
    meeting, memory,
    persistence::{SqliteReaders, SqliteWriter},
    situation, voice, AppState,
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex},
};

/// An AppState that shares an externally built writer and capability service (for tests that
/// need a ready, active generated capability). Publication stays disabled by default.
pub(crate) fn app_state_with_capabilities(
    sqlite_writer: Arc<SqliteWriter>,
    generated_capabilities: Arc<CapabilityService>,
) -> AppState {
    let settings = sqlite_writer
        .read_serialized(|connection| {
            situation::repository::load_settings(connection)
                .map_err(|error| format!("Situation settings load: {error}"))
        })
        .expect("Situation settings load");
    let sqlite_readers = SqliteReaders::serialized(sqlite_writer.clone());
    let tool_selection = Arc::new(crate::tool_selection::build_service(
        sqlite_writer.clone(),
        &crate::tool_selection::ToolSelectionConfig::direct(),
        Some(generated_capabilities.clone()),
    ));
    AppState {
        sqlite_writer,
        sqlite_readers,
        data_directory: PathBuf::new(),
        context_still_recall: memory::context_still_recall::ContextStillRecallClient::disabled(),
        active_runs: Mutex::new(HashMap::new()),
        provider_probes: Mutex::new(HashMap::new()),
        interaction_policy: Mutex::new(()),
        shutdown_started: AtomicBool::new(false),
        network_asr: voice::network_asr::NetworkAsrRuntime::new()
            .expect("Network ASR runtime initializes"),
        audio_uploads: voice::audio_upload::AudioUploadStore::default(),
        streaming_tts: voice::streaming_tts::runtime::StreamingSpeechRuntime::default(),
        voice_behavior: crate::voice_behavior::VoiceBehaviorRuntime::default(),
        situation: Arc::new(
            situation::SituationRuntime::new(settings, None)
                .expect("Situation runtime initializes"),
        ),
        meeting: Arc::new(meeting::MeetingRuntime::new()),
        voice_profile: Arc::new(voice::profile::VoiceProfileRuntime::unavailable_for_tests(
            PathBuf::new(),
        )),
        voice_asr: voice::streaming_asr::AsrSessionManager::default(),
        generated_capabilities,
        generated_tools: GeneratedToolsConfig::disabled(),
        tool_selection,
        mcp_server: std::sync::Mutex::new(None),
    }
}
