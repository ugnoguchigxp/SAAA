use crate::{
    meeting, memory,
    persistence::{settings::default_settings_documents, SqliteReaders, SqliteWriter},
    providers, situation, voice, AppState, DynamicLanProviderSettings, LarmProviderSettings,
    ModelProviderSettings, OpenAiCompatibleProviderSettings, SaveSettingsDocumentInput,
};
use rusqlite::Connection;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex},
};

pub(crate) fn app_state(connection: Connection) -> AppState {
    let settings =
        situation::repository::load_settings(&connection).expect("Situation settings load");
    let sqlite_writer = Arc::new(SqliteWriter::from_connection(connection));
    let sqlite_readers = SqliteReaders::serialized(sqlite_writer.clone());
    AppState {
        sqlite_writer,
        sqlite_readers,
        data_directory: PathBuf::new(),
        context_still_recall: memory::context_still_recall::ContextStillRecallClient::disabled(),
        active_runs: Mutex::new(HashMap::new()),
        provider_probes: Mutex::new(HashMap::new()),
        interaction_policy: Mutex::new(()),
        shutdown_started: AtomicBool::new(false),
        larm_gate: providers::larm::LarmRuntimeGate::Disabled,
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
    }
}

pub(crate) fn provider(id: &str, location: &str) -> ModelProviderSettings {
    ModelProviderSettings::OpenAiCompatible(direct_provider(id, location))
}

pub(crate) fn direct_provider(id: &str, location: &str) -> OpenAiCompatibleProviderSettings {
    OpenAiCompatibleProviderSettings {
        id: id.to_string(),
        enabled: true,
        label: id.to_string(),
        location: location.to_string(),
        endpoint: if location == "local" {
            "http://127.0.0.1:11434/v1".to_string()
        } else {
            "https://example.invalid/v1".to_string()
        },
        model: "test-model".to_string(),
        authentication: "none".to_string(),
    }
}

pub(crate) fn larm_provider(id: &str) -> ModelProviderSettings {
    ModelProviderSettings::Larm(LarmProviderSettings {
        id: id.to_string(),
        enabled: true,
        label: id.to_string(),
        location: "local".to_string(),
        base_url: "http://127.0.0.1:9810/".to_string(),
        token_env: "LARM_API_TOKEN".to_string(),
        allocation_ttl_seconds: 300,
        allocation_startup_timeout_seconds: 300,
        allow_fallback_by_default: false,
        deployment_policy: "existing-only".to_string(),
    })
}

pub(crate) fn dynamic_lan_provider(id: &str) -> ModelProviderSettings {
    ModelProviderSettings::DynamicLan(DynamicLanProviderSettings {
        id: id.to_string(),
        enabled: true,
        label: id.to_string(),
        location: "local".to_string(),
        host: "10.0.0.42".to_string(),
    })
}

pub(crate) fn default_settings_input() -> Vec<SaveSettingsDocumentInput> {
    default_settings_documents()
        .into_iter()
        .map(
            |(namespace, key, schema_version, value_json)| SaveSettingsDocumentInput {
                namespace: namespace.to_string(),
                key: key.to_string(),
                schema_version,
                value_json,
            },
        )
        .collect()
}
