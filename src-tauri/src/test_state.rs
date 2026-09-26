//! Test-only AppState construction. Kept in its own module so the provider helpers in
//! `test_support` stay small and the shared writer/capability-service variant is explicit.

use crate::{
    generated_capabilities::{publication::GeneratedToolsConfig, service::CapabilityService},
    memory,
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
        context_still_search: memory::context_still_search::ContextStillSearchClient::disabled(),
        active_runs: Mutex::new(HashMap::new()),
        provider_probes: Mutex::new(HashMap::new()),
        interaction_policy: Mutex::new(()),
        shutdown_started: AtomicBool::new(false),
        audio_uploads: voice::audio_upload::AudioUploadStore::default(),
        streaming_tts: voice::unavailable_speech::UnavailableSpeechRuntime,
        voice_behavior: crate::voice_behavior::VoiceBehaviorRuntime::default(),
        situation: Arc::new(
            situation::SituationRuntime::new(settings, None)
                .expect("Situation runtime initializes"),
        ),
        voice_profile: Arc::new(voice::profile::VoiceProfileRuntime::unavailable_for_tests(
            PathBuf::new(),
        )),
        generated_capabilities,
        generation: None,
        generated_tools: GeneratedToolsConfig::disabled(),
        tool_selection,
        mcp_server: std::sync::Mutex::new(None),
        schedule: Arc::new(crate::schedule::Handle::default()),
        steward_wake: crate::steward::pump::Wake::default(),
        artifact_preview: crate::artifact_preview::PreviewRuntime::default(),
        reachability: std::sync::Arc::new(
            crate::providers::reachability::ReachabilityState::default(),
        ),
        reachability_kick: std::sync::Arc::new(tokio::sync::Notify::new()),
        diagnosis: std::sync::Arc::new(crate::diagnosis::store::DiagnosisStore::new()),
        context_segments_enabled: std::env::var("SAAA_CONTEXT_SEGMENTS").ok().as_deref()
            == Some("1"),
        wire_prefixes: Mutex::new(std::collections::VecDeque::new()),
    }
}
