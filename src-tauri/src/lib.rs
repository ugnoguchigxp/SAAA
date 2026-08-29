use futures_util::StreamExt;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{hash_map::Entry, HashMap},
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread,
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::Manager;
use zeroize::Zeroizing;

mod backup;
mod diagnostics;
pub mod ipc_contract;
mod meeting;
mod memory;
mod persistence;
mod process_guard;
mod providers;
mod redact;
mod runtime;
mod situation;
#[cfg(test)]
mod test_support;
mod voice;

use backup::backup_connection_to;
use providers::openai_compatible::{
    drain_sse_events, probe_model_provider, provider_api_key, provider_chat_url, SseDrainError,
};

use persistence::{
    backup_before_migration, ensure_primary_conversation, initialize_database,
    list_conversations_from_connection, list_messages_from_connection, list_settings_documents,
    save_settings_documents_to_connection, validate_conversation_write_target,
    validate_model_providers, validate_settings_batch, validate_settings_document,
};
pub(crate) use providers::openai_compatible::provider_environment_suffix;
pub(crate) use providers::session_store::{
    begin_provider_session, finish_gnosis_provider_session, finish_larm_provider_session,
    finish_provider_session, mark_larm_release_pending, mark_provider_output_started,
    persist_conversation_success, persist_larm_request_id, persist_larm_selection,
};
pub(crate) use runtime::codex_turn::execute_codex_turn;
#[cfg(test)]
pub(crate) use runtime::codex_turn::{
    persist_codex_thread, receive_supervised_codex_result, run_codex_turn_process,
    run_codex_turn_process_with_policy,
};
#[cfg(test)]
pub(crate) use runtime::turns::prepare_runtime_run;
pub(crate) use runtime::turns::{execute_turn, finish_runtime_run, send_runtime_terminal_event};

use diagnostics::build_provider_diagnostics;
use process_guard::ProcessGuard;
use redact::{bounded_text, redact_runtime_text};

use ipc_contract::{ConversationMessage, RuntimeEvent};

static BUNDLED_CODEX_PATH: OnceLock<PathBuf> = OnceLock::new();
const MAX_CODEX_STDOUT_BYTES: u64 = 4 * 1_024 * 1_024;
const WINDOW_SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const GNOSIS_PROVIDER_ID: &str = "gnosis-qwen";
const GNOSIS_HOST: &str = "192.168.0.65";
const PRIMARY_CONVERSATION_ID: &str = "conversation_primary";
const PRIMARY_CONVERSATION_TITLE: &str = "SAAAとの会話";
const CODEX_READ_ONLY_SYSTEM_CONTEXT: &str = include_str!("../../.s11tnext/codex-read-only.txt");

struct AppState {
    connection: Arc<Mutex<Connection>>,
    data_directory: PathBuf,
    context_still_recall: memory::context_still_recall::ContextStillRecallClient,
    active_runs: Mutex<HashMap<String, Arc<RunCancellation>>>,
    interaction_policy: Mutex<()>,
    shutdown_started: AtomicBool,
    larm_gate: providers::larm::LarmRuntimeGate,
    tts_process: Mutex<Option<ActiveTts>>,
    situation: Arc<situation::SituationRuntime>,
    meeting: Arc<meeting::MeetingRuntime>,
    voice_profile: Arc<voice::profile::VoiceProfileRuntime>,
}

struct ActiveTts {
    run_id: String,
    child: Child,
}

#[derive(Default)]
struct RunCancellation {
    cancelled: AtomicBool,
    notify: tokio::sync::Notify,
}

impl RunCancellation {
    pub(crate) fn cancel(&self) {
        if !self.cancelled.swap(true, Ordering::SeqCst) {
            self.notify.notify_waiters();
            self.notify.notify_one();
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    async fn cancelled(&self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsDocument {
    namespace: String,
    key: String,
    schema_version: i64,
    value_json: Value,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveSettingsDocumentInput {
    namespace: String,
    key: String,
    schema_version: i64,
    value_json: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveSettingsDocumentsInput {
    documents: Vec<SaveSettingsDocumentInput>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Conversation {
    id: String,
    title: Option<String>,
    task_mode: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateConversationInput {
    title: Option<String>,
    task_mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AppendMessageInput {
    conversation_id: String,
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSnapshot {
    settings: Vec<SettingsDocument>,
    conversations: Vec<Conversation>,
    primary_conversation_id: String,
    larm_runtime: LarmRuntimeStatus,
    voice_profile: voice::profile::VoiceProfileSnapshot,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LarmRuntimeStatus {
    state: &'static str,
    message: &'static str,
    contract_commit: &'static str,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexReasoningEffort {
    reasoning_effort: String,
    description: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexModelOption {
    id: String,
    model: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    default_reasoning_effort: Option<String>,
    #[serde(default)]
    supported_reasoning_efforts: Vec<CodexReasoningEffort>,
    #[serde(default = "default_codex_input_modalities")]
    input_modalities: Vec<String>,
    #[serde(default)]
    supports_personality: bool,
    #[serde(default)]
    is_default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexModelPage {
    data: Vec<CodexModelOption>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CodexRuntimeStatus {
    installed: bool,
    authenticated: bool,
    runtime: String,
    account_type: Option<String>,
    message: String,
}

#[derive(Debug)]
struct CodexTurnOutcome {
    thread_id: String,
    content: String,
    last_progress_at: Option<String>,
}

#[derive(Debug)]
struct CodexTurnFailure {
    thread_id: Option<String>,
    message: String,
    code: runtime::contracts::RunFailureCode,
    last_progress_at: Option<String>,
}

#[derive(Debug)]
enum CodexReaderMessage {
    Message(Value),
    Failed {
        code: runtime::contracts::RunFailureCode,
        message: &'static str,
    },
}

#[derive(Debug)]
struct TurnCompletion;

#[derive(Debug)]
struct TurnExecutionFailure {
    code: runtime::contracts::RunFailureCode,
    message: String,
    supervisor_version: Option<&'static str>,
    last_progress_at: Option<String>,
    finalized: bool,
}

impl TurnExecutionFailure {
    fn unsupervised(code: runtime::contracts::RunFailureCode, message: String) -> Self {
        Self {
            code,
            message,
            supervisor_version: None,
            last_progress_at: None,
            finalized: false,
        }
    }

    fn configuration(message: impl Into<String>) -> Self {
        Self::unsupervised(
            runtime::contracts::RunFailureCode::ConfigurationError,
            message.into(),
        )
    }
}

impl From<String> for TurnExecutionFailure {
    fn from(message: String) -> Self {
        Self::unsupervised(runtime::contracts::RunFailureCode::InternalError, message)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OpenAiCompatibleProviderSettings {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) label: String,
    pub(crate) location: String,
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) credential_status: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LarmProviderSettings {
    id: String,
    enabled: bool,
    label: String,
    location: String,
    base_url: String,
    token_env: String,
    allocation_ttl_seconds: u32,
    allocation_startup_timeout_seconds: u32,
    allow_fallback_by_default: bool,
    deployment_policy: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GnosisProviderSettings {
    id: String,
    enabled: bool,
    label: String,
    location: String,
    host: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum ModelProviderSettings {
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible(OpenAiCompatibleProviderSettings),
    #[serde(rename = "larm")]
    Larm(LarmProviderSettings),
    #[serde(rename = "gnosis")]
    Gnosis(GnosisProviderSettings),
}

impl ModelProviderSettings {
    fn id(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.id,
            Self::Larm(provider) => &provider.id,
            Self::Gnosis(provider) => &provider.id,
        }
    }

    fn enabled(&self) -> bool {
        match self {
            Self::OpenAiCompatible(provider) => provider.enabled,
            Self::Larm(provider) => provider.enabled,
            Self::Gnosis(provider) => provider.enabled,
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.label,
            Self::Larm(provider) => &provider.label,
            Self::Gnosis(provider) => &provider.label,
        }
    }

    fn location(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.location,
            Self::Larm(provider) => &provider.location,
            Self::Gnosis(provider) => &provider.location,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible(_) => "openai-compatible",
            Self::Larm(_) => "larm",
            // A resolved gnosis descriptor executes through the OpenAI-compatible
            // data plane; keep the persisted session kind compatible with the
            // existing provider-session schema.
            Self::Gnosis(_) => "openai-compatible",
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        match self {
            Self::OpenAiCompatible(provider) => provider.enabled = enabled,
            Self::Larm(provider) => provider.enabled = enabled,
            Self::Gnosis(provider) => provider.enabled = enabled,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelProvidersSettings {
    providers: Vec<ModelProviderSettings>,
    #[serde(default = "providers::default_conversation_reasoning_effort")]
    reasoning_effort: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct ConversationRouteSettings {
    primary_provider_id: String,
    fallback_provider_ids: Vec<String>,
    timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct CodingRouteSettings {
    provider_id: String,
    timeout_ms: u64,
    read_only: bool,
    network_enabled: bool,
    web_search_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct RoutingSettings {
    conversation_respond: ConversationRouteSettings,
    coding_assist: CodingRouteSettings,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct CodexAgentRuntimeSettings {
    enabled: bool,
    provider: String,
    model: String,
    runtime_mode: String,
    health: String,
    sandbox_mode: String,
    approval_policy: String,
    network_enabled: bool,
    web_search_enabled: bool,
    workspace_policy: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct VoiceRuntimeSettings {
    input_device_id: String,
    output_device_id: String,
    capture_mode: String,
    stt_provider_id: String,
    stt_model: String,
    tts_provider_id: String,
    tts_voice: String,
    auto_speak: bool,
    cloud_fallback_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct SecurityRuntimeSettings {
    credential_storage: String,
    local_only_when_selected: bool,
    diagnostics_redaction: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartTurnInput {
    run_id: String,
    conversation_id: String,
    content: String,
    workspace_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TestProviderInput {
    provider: ModelProviderSettings,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderTestResult {
    provider_id: String,
    ok: bool,
    message: String,
    latency_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalArtifactResult {
    path: String,
    created_at: String,
}

#[tauri::command]
fn frontend_ready(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let Some(marker_id) = env::var("SAAA_SMOKE_MARKER_ID")
        .ok()
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    validate_identifier(&marker_id, "smoke marker id")?;
    if env::var_os("SAAA_SMOKE_REQUIRE_SPEAKER").is_some() {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        let voice_profile = state.voice_profile.snapshot(&connection)?;
        if !voice_profile.runtime_available {
            return Err(format!(
                "Packaged speaker verification is unavailable: {}",
                voice_profile.runtime_message
            ));
        }
    }
    if env::var_os("SAAA_SMOKE_EXERCISE_SITUATION").is_some() {
        state.situation.set_monitoring(&state.connection, true)?;
        let sample = state.situation.sample_platform()?;
        state.situation.tick_sampled(&state.connection, sample)?;
        state.situation.set_monitoring(&state.connection, false)?;
    }
    fs::write(
        env::temp_dir().join(format!("saaa-frontend-{marker_id}.ready")),
        "ready",
    )
    .map_err(|error| format!("Could not write the frontend smoke marker: {error}"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TranscribeAudioInput {
    run_id: String,
    conversation_id: String,
    samples: Vec<f32>,
    sample_rate: u32,
    model: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewAudioInput {
    run_id: String,
    conversation_id: String,
    samples: Vec<f32>,
    sample_rate: u32,
    model: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SpeakTextInput {
    run_id: String,
    conversation_id: String,
    text: String,
    voice: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum VoiceEvent {
    Transcribing {
        run_id: String,
    },
    TranscriptFinal {
        run_id: String,
        text: String,
    },
    TranscriptDelta {
        run_id: String,
        text: String,
    },
    Cancelled {
        run_id: String,
    },
    Failed {
        run_id: String,
        message: String,
        recovery: String,
    },
}

#[tauri::command]
fn get_app_snapshot(state: tauri::State<'_, AppState>) -> Result<AppSnapshot, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let primary_conversation = ensure_primary_conversation(&connection)?;
    let mut conversations = list_conversations_from_connection(&connection)?;
    if !conversations
        .iter()
        .any(|conversation| conversation.id == primary_conversation.id)
    {
        conversations.push(primary_conversation.clone());
    }
    Ok(AppSnapshot {
        settings: list_settings_documents(&connection)?,
        conversations,
        primary_conversation_id: primary_conversation.id,
        larm_runtime: LarmRuntimeStatus {
            state: state.larm_gate.state(),
            message: state.larm_gate.public_message(),
            contract_commit: providers::larm::CONTRACT_COMMIT,
        },
        voice_profile: state.voice_profile.snapshot(&connection)?,
    })
}

#[tauri::command]
fn get_voice_profile_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<voice::profile::VoiceProfileSnapshot, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    state.voice_profile.snapshot(&connection)
}

#[tauri::command]
fn save_voice_enrollment_sample(
    state: tauri::State<'_, AppState>,
    input: voice::profile::SaveVoiceEnrollmentSampleInput,
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
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    state.voice_profile.save_sample(&connection, input)
}

#[tauri::command]
fn set_target_speaker_filter_enabled(
    state: tauri::State<'_, AppState>,
    input: voice::profile::SetTargetSpeakerFilterInput,
) -> Result<voice::profile::VoiceProfileSnapshot, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    state
        .voice_profile
        .set_filter_enabled(&connection, input.enabled)
}

#[tauri::command]
fn delete_voice_enrollment_sample(
    state: tauri::State<'_, AppState>,
    sample_id: String,
) -> Result<voice::profile::VoiceProfileSnapshot, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    state.voice_profile.delete_sample(&connection, &sample_id)
}

#[tauri::command]
fn delete_voice_profile(
    state: tauri::State<'_, AppState>,
) -> Result<voice::profile::VoiceProfileSnapshot, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    state.voice_profile.delete_profile(&connection)
}

#[tauri::command]
fn read_voice_enrollment_sample(
    state: tauri::State<'_, AppState>,
    sample_id: String,
) -> Result<Vec<u8>, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    state.voice_profile.read_sample(&connection, &sample_id)
}

#[tauri::command]
fn get_situation_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<situation::contracts::SituationSnapshot, String> {
    state.situation.snapshot_locked(&state.connection)
}

#[tauri::command]
fn set_situation_monitoring(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<situation::contracts::SituationSnapshot, String> {
    state.situation.set_monitoring(&state.connection, enabled)?;
    if enabled {
        spawn_situation_monitor(state.connection.clone(), state.situation.clone());
    }
    get_situation_snapshot(state)
}

#[tauri::command]
fn report_owned_signal(
    state: tauri::State<'_, AppState>,
    input: situation::contracts::OwnedSignalInput,
) -> Result<(), String> {
    state.situation.report_owned(input)
}

#[tauri::command]
fn submit_situation_feedback(
    state: tauri::State<'_, AppState>,
    input: situation::contracts::SituationFeedbackInput,
) -> Result<situation::contracts::SituationSnapshot, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    situation::repository::submit_feedback(&connection, &input)?;
    drop(connection);
    state.situation.snapshot_locked(&state.connection)
}

#[tauri::command]
fn clear_situation_history(
    state: tauri::State<'_, AppState>,
) -> Result<situation::contracts::SituationSnapshot, String> {
    state.situation.clear_history(&state.connection)?;
    state.situation.snapshot_locked(&state.connection)
}

#[tauri::command]
fn get_situation_review_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<situation::contracts::SituationReviewSnapshot, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    Ok(situation::contracts::SituationReviewSnapshot {
        active_profile: situation::calibration::active_profile(&connection)?,
        quality: situation::repository::quality_metrics(&connection)?,
        feedback_queue: situation::repository::feedback_queue(&connection)?,
        latest_run: situation::calibration::latest_run(&connection)?,
        candidates: situation::calibration::candidates(&connection)?,
    })
}

#[tauri::command]
fn create_situation_calibration_candidate(
    state: tauri::State<'_, AppState>,
    parameters: situation::contracts::CalibrationParameters,
) -> Result<situation::calibration::CalibrationProfile, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    situation::calibration::create_candidate(&connection, parameters)
}

#[tauri::command]
async fn run_situation_calibration(
    state: tauri::State<'_, AppState>,
    profile_id: String,
) -> Result<situation::calibration::CalibrationRun, String> {
    validate_identifier(&profile_id, "calibration profile id")?;
    let connection = state.connection.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let connection = connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        let profile = situation::calibration::profile_by_id(&connection, &profile_id)?;
        if profile.status != "candidate" {
            return Err("Only candidate profiles can be replayed".to_string());
        }
        let metrics = situation::calibration::replay_metrics(&profile)?;
        situation::calibration::save_run(&connection, &profile_id, "completed", Some(metrics), None)
    })
    .await
    .map_err(|error| format!("Situation calibration worker failed: {error}"))?
}

#[tauri::command]
fn decide_situation_calibration(
    state: tauri::State<'_, AppState>,
    profile_id: String,
    decision: String,
    reason_code: String,
) -> Result<situation::contracts::SituationReviewSnapshot, String> {
    state
        .situation
        .decide_calibration(&state.connection, &profile_id, &decision, &reason_code)?;
    get_situation_review_snapshot(state)
}

fn spawn_situation_monitor(
    connection: Arc<Mutex<Connection>>,
    runtime: Arc<situation::SituationRuntime>,
) {
    if !runtime.enabled() || !runtime.begin_worker() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        while runtime.enabled() {
            let result = runtime
                .sample_platform()
                .and_then(|sample| runtime.tick_sampled(&connection, sample));
            if let Err(error) = result {
                runtime.record_failure(error);
            }
            runtime.wait_for_next_sample().await;
        }
        runtime.finish_worker();
        if runtime.enabled() {
            spawn_situation_monitor(connection, runtime);
        }
    });
}

#[tauri::command]
fn export_diagnostics(state: tauri::State<'_, AppState>) -> Result<LocalArtifactResult, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let settings = list_settings_documents(&connection)?
        .into_iter()
        .map(|document| {
            json!({
                "namespace": document.namespace,
                "key": document.key,
                "schemaVersion": document.schema_version,
                "updatedAt": document.updated_at
            })
        })
        .collect::<Vec<_>>();
    let conversation_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM conversations", [], |row| row.get(0))
        .map_err(database_error)?;
    let message_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM conversation_messages", [], |row| {
            row.get(0)
        })
        .map_err(database_error)?;
    let mut statement = connection
        .prepare(
            "SELECT route_kind, COALESCE(provider_id, ''), status, COALESCE(error_message, '')
             FROM runtime_runs ORDER BY started_at DESC LIMIT 20",
        )
        .map_err(database_error)?;
    let recent_runs = statement
        .query_map([], |row| {
            Ok(json!({
                "route": row.get::<_, String>(0)?,
                "providerId": row.get::<_, String>(1)?,
                "status": row.get::<_, String>(2)?,
                "error": redact_runtime_text(&row.get::<_, String>(3)?)
            }))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    let provider_diagnostics = build_provider_diagnostics(&connection)?;
    let situation_evaluation = situation::repository::evaluation_summary(&connection)?;
    let situation_settings = situation::repository::load_settings(&connection)?;
    let situation_profile = situation::calibration::active_profile(&connection)?;
    let latest_calibration = situation::calibration::latest_run(&connection)?;
    drop(connection);

    let created_at = now_iso();
    let payload = json!({
        "format": "saaa-diagnostics-v1",
        "createdAt": created_at,
        "redacted": true,
        "application": { "version": env!("CARGO_PKG_VERSION"), "platform": env::consts::OS, "arch": env::consts::ARCH },
        "database": { "settingsDocuments": settings, "conversationCount": conversation_count, "messageCount": message_count },
        "situation": {
            "monitoringEnabled": situation_settings.enabled,
            "calendarEnabled": situation_settings.calendar_enabled,
            "activeRuleVersion": situation_profile.rule_version,
            "latestCalibrationStatus": latest_calibration.as_ref().map(|run| run.status.as_str()),
            "totalEntries": situation_evaluation.total_entries,
            "feedback": {
                "accurate": situation_evaluation.accurate,
                "inaccurate": situation_evaluation.inaccurate,
                "unsure": situation_evaluation.unsure
            }
        },
        "recentRuns": recent_runs,
        "providerSessions": provider_diagnostics
    });
    let directory = state.data_directory.join("diagnostics");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the diagnostics directory: {error}"))?;
    let path = directory.join(format!("saaa-diagnostics-{created_at}.json"));
    let contents = serde_json::to_vec_pretty(&payload)
        .map_err(|error| format!("Could not encode diagnostics: {error}"))?;
    fs::write(&path, contents).map_err(|error| format!("Could not write diagnostics: {error}"))?;
    Ok(LocalArtifactResult {
        path: path.to_string_lossy().into_owned(),
        created_at,
    })
}

#[tauri::command]
fn backup_database(state: tauri::State<'_, AppState>) -> Result<LocalArtifactResult, String> {
    let created_at = now_iso();
    let directory = state.data_directory.join("backups");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the backup directory: {error}"))?;
    let path = directory.join(format!("saaa-{created_at}.sqlite3"));
    let source = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    backup_connection_to(&source, &path)?;
    Ok(LocalArtifactResult {
        path: path.to_string_lossy().into_owned(),
        created_at,
    })
}

#[tauri::command]
async fn start_turn(
    state: tauri::State<'_, AppState>,
    input: StartTurnInput,
    on_event: tauri::ipc::Channel<RuntimeEvent>,
) -> Result<(), String> {
    validate_start_turn(&input)?;
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(&state, &input.run_id, cancellation.clone())?;

    let result = execute_turn(&state, &input, &on_event, cancellation.clone(), None).await;
    if let Ok(mut active) = state.active_runs.lock() {
        active.remove(&input.run_id);
    }
    result.map_err(|error| redact_runtime_text(&error.message))
}

#[tauri::command]
fn cancel_run(state: tauri::State<'_, AppState>, run_id: String) -> Result<(), String> {
    validate_identifier(&run_id, "run id")?;
    let active = state
        .active_runs
        .lock()
        .map_err(|_| "Runtime run lock unavailable".to_string())?;
    if let Some(cancellation) = active.get(&run_id) {
        cancellation.cancel();
    }
    Ok(())
}

#[tauri::command]
async fn test_model_provider(
    state: tauri::State<'_, AppState>,
    input: TestProviderInput,
) -> Result<ProviderTestResult, String> {
    let mut provider = input.provider;
    provider.set_enabled(true);
    validate_identifier(provider.id(), "provider id")?;
    validate_model_providers(&ModelProvidersSettings {
        providers: vec![provider.clone()],
        reasoning_effort: providers::default_conversation_reasoning_effort(),
    })?;
    let started = std::time::Instant::now();
    let result = match &provider {
        ModelProviderSettings::OpenAiCompatible(provider) => probe_model_provider(provider).await,
        ModelProviderSettings::Larm(provider) => {
            providers::larm::LarmProvider::probe(&state.larm_gate, &provider.base_url)
                .await
                .map(|_| "LARM health and readiness checks succeeded".to_string())
                .map_err(|kind| larm_failure_message(kind).to_string())
        }
        ModelProviderSettings::Gnosis(provider) => {
            match providers::gnosis::GnosisConnection::resolve(
                &provider.host,
                Arc::new(RunCancellation::default()),
            )
            .await
            {
                Ok(connection) => {
                    let message = format!(
                        "gnosis dynamically resolved model {} at {}",
                        connection.model(),
                        connection.endpoint()
                    );
                    connection
                        .release()
                        .await
                        .map(|_| message)
                        .map_err(|error| error.public_message().to_string())
                }
                Err(error) => Err(error.public_message().to_string()),
            }
        }
    };
    Ok(ProviderTestResult {
        provider_id: provider.id().to_string(),
        ok: result.is_ok(),
        message: result.unwrap_or_else(|error| redact_runtime_text(&error)),
        latency_ms: started.elapsed().as_millis(),
    })
}

#[tauri::command]
async fn transcribe_audio(
    state: tauri::State<'_, AppState>,
    mut input: TranscribeAudioInput,
    on_event: tauri::ipc::Channel<VoiceEvent>,
) -> Result<String, String> {
    validate_identifier(&input.run_id, "run id")?;
    validate_identifier(&input.conversation_id, "conversation id")?;
    if !(8_000..=192_000).contains(&input.sample_rate) || input.samples.is_empty() {
        return Err("Recorded audio is empty or has an unsupported sample rate".to_string());
    }
    if input.samples.iter().any(|sample| !sample.is_finite()) {
        return Err("Recorded audio contains invalid samples".to_string());
    }
    if input.samples.len() > input.sample_rate as usize * 300 {
        return Err("Recording exceeds the five minute MVP limit".to_string());
    }
    if input.model != voice::gnosis_asr::MODEL_ID {
        return Err("Voice settings must use the configured gnosis ASR model".to_string());
    }
    let samples = Zeroizing::new(std::mem::take(&mut input.samples));
    let verification = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        state
            .voice_profile
            .verify_if_enabled(&connection, &samples, input.sample_rate)
    };
    if let Err(error) = verification {
        let recovery = "Use the enrolled microphone and speak clearly, or disable the target-speaker filter in Settings.".to_string();
        let _ = on_event.send(VoiceEvent::Failed {
            run_id: input.run_id.clone(),
            message: error.clone(),
            recovery,
        });
        return Err(error);
    }
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(&state, &input.run_id, cancellation.clone())?;
    if let Err(error) = begin_simple_runtime_run(
        &state,
        &input.run_id,
        &input.conversation_id,
        "voice.transcribe",
        voice::gnosis_asr::PROVIDER_ID,
    ) {
        remove_active_run(&state, &input.run_id);
        state
            .situation
            .set_microphone_state(situation::contracts::MicrophoneState::Inactive);
        return Err(error);
    }
    let _ = on_event.send(VoiceEvent::Transcribing {
        run_id: input.run_id.clone(),
    });
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::SaaaTranscribing);
    let result = voice::gnosis_asr::transcribe(
        &samples,
        input.sample_rate,
        &input.model,
        cancellation.clone(),
    )
    .await
    .map(|(text, _language)| text);
    remove_active_run(&state, &input.run_id);
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::Inactive);
    match result {
        Ok(transcript) => {
            finish_runtime_run(&state, &input.run_id, "completed", None)?;
            let _ = on_event.send(VoiceEvent::TranscriptFinal {
                run_id: input.run_id,
                text: transcript.clone(),
            });
            Ok(transcript)
        }
        Err(error) if cancellation.is_cancelled() => {
            finish_runtime_run(
                &state,
                &input.run_id,
                "cancelled",
                Some("Cancelled by user"),
            )?;
            let _ = on_event.send(VoiceEvent::Cancelled {
                run_id: input.run_id,
            });
            Err("Transcription cancelled".to_string())
        }
        Err(error) => {
            let error = redact_runtime_text(&error);
            finish_runtime_run(&state, &input.run_id, "failed", Some(&error))?;
            let _ = on_event.send(VoiceEvent::Failed {
                run_id: input.run_id,
                message: error.clone(),
                recovery: "Check the gnosis ASR service and retry.".to_string(),
            });
            Err(error)
        }
    }
}

#[tauri::command]
async fn preview_audio(
    state: tauri::State<'_, AppState>,
    mut input: PreviewAudioInput,
    on_event: tauri::ipc::Channel<VoiceEvent>,
) -> Result<String, String> {
    validate_identifier(&input.run_id, "run id")?;
    validate_identifier(&input.conversation_id, "conversation id")?;
    if !(8_000..=192_000).contains(&input.sample_rate)
        || input.samples.len() < input.sample_rate as usize
        || input.samples.len() > input.sample_rate as usize * 15
        || input.samples.iter().any(|sample| !sample.is_finite())
    {
        return Err(
            "Voice preview must contain between one and fifteen seconds of valid audio".to_string(),
        );
    }
    if input.model != voice::gnosis_asr::MODEL_ID {
        return Err("Voice settings must use the configured gnosis ASR model".to_string());
    }
    let samples = Zeroizing::new(std::mem::take(&mut input.samples));
    {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        state
            .voice_profile
            .verify_if_enabled(&connection, &samples, input.sample_rate)?;
    }
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(&state, &input.run_id, cancellation.clone())?;
    let result = voice::gnosis_asr::transcribe(
        &samples,
        input.sample_rate,
        &input.model,
        cancellation.clone(),
    )
    .await
    .map(|(text, _language)| text);
    remove_active_run(&state, &input.run_id);
    match result {
        Ok(transcript) => {
            let _ = on_event.send(VoiceEvent::TranscriptDelta {
                run_id: input.run_id,
                text: transcript.clone(),
            });
            Ok(transcript)
        }
        Err(_) if cancellation.is_cancelled() => Err("Voice preview cancelled".to_string()),
        Err(error) => Err(redact_runtime_text(&error)),
    }
}

#[tauri::command]
async fn speak_text(
    state: tauri::State<'_, AppState>,
    input: SpeakTextInput,
) -> Result<(), String> {
    validate_identifier(&input.run_id, "run id")?;
    validate_identifier(&input.conversation_id, "conversation id")?;
    if input.text.trim().is_empty() || input.text.chars().count() > 16_000 {
        return Err("Speech text must contain between 1 and 16,000 characters".to_string());
    }
    if input.voice.trim().is_empty() || input.voice.len() > 160 {
        return Err("TTS voice must contain between 1 and 160 characters".to_string());
    }
    let speech_text = voice::tts::text_for_speech(&input.text);
    if speech_text.is_empty() {
        return Ok(());
    }
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(&state, &input.run_id, cancellation.clone())?;
    if let Err(error) = begin_simple_runtime_run(
        &state,
        &input.run_id,
        &input.conversation_id,
        "voice.speak",
        "system-tts",
    ) {
        remove_active_run(&state, &input.run_id);
        return Err(error);
    }

    let spawn_result = (|| {
        let mut process = state
            .tts_process
            .lock()
            .map_err(|_| "TTS process lock unavailable".to_string())?;
        if process.is_some() {
            return Err("Another speech run is already active".to_string());
        }
        if state.meeting.blocks_tts() {
            return Err(
                "MEETING_POLICY_TTS_BLOCKED: Speech is disabled during a meeting.".to_string(),
            );
        }
        *process = Some(ActiveTts {
            run_id: input.run_id.clone(),
            child: spawn_tts_process(&speech_text, &input.voice)?,
        });
        Ok::<(), String>(())
    })();
    if let Err(error) = spawn_result {
        remove_active_run(&state, &input.run_id);
        finish_runtime_run(&state, &input.run_id, "failed", Some(&error))?;
        return Err(error);
    }
    state
        .situation
        .set_audio_state(situation::contracts::AudioState::SaaaSpeaking);

    let result: Result<(), String> = async {
        loop {
            if cancellation.is_cancelled() {
                break Err("Speech cancelled".to_string());
            }
            let status = {
                let mut process = state
                    .tts_process
                    .lock()
                    .map_err(|_| "TTS process lock unavailable".to_string())?;
                let active = process
                    .as_mut()
                    .filter(|active| active.run_id == input.run_id)
                    .ok_or_else(|| "Speech process ownership was lost".to_string())?;
                active
                    .child
                    .try_wait()
                    .map_err(|error| format!("Could not inspect TTS process: {error}"))?
            };
            if let Some(status) = status {
                if status.success() {
                    break Ok(());
                }
                break Err(format!("System TTS exited with {status}"));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    .await;

    if let Ok(mut process) = state.tts_process.lock() {
        if process
            .as_ref()
            .is_some_and(|active| active.run_id == input.run_id)
        {
            if let Some(mut active) = process.take() {
                if cancellation.is_cancelled() {
                    let _ = active.child.kill();
                }
                let _ = active.child.wait();
            }
            state
                .situation
                .set_audio_state(situation::contracts::AudioState::Silent);
        }
    }
    remove_active_run(&state, &input.run_id);
    match result {
        Ok(()) => finish_runtime_run(&state, &input.run_id, "completed", None),
        Err(error) if cancellation.is_cancelled() => {
            finish_runtime_run(&state, &input.run_id, "cancelled", Some(&error))?;
            Err(error)
        }
        Err(error) => {
            finish_runtime_run(&state, &input.run_id, "failed", Some(&error))?;
            Err(error)
        }
    }
}

#[tauri::command]
fn stop_tts(state: tauri::State<'_, AppState>, run_id: String) -> Result<(), String> {
    validate_identifier(&run_id, "run id")?;
    let mut process = state
        .tts_process
        .lock()
        .map_err(|_| "TTS process lock unavailable".to_string())?;
    if process
        .as_ref()
        .is_none_or(|active| active.run_id != run_id)
    {
        return Ok(());
    }
    {
        let active_runs = state
            .active_runs
            .lock()
            .map_err(|_| "Runtime run lock unavailable".to_string())?;
        if let Some(cancellation) = active_runs.get(&run_id) {
            cancellation.cancel();
        }
    }
    if let Some(mut active) = process.take() {
        let _ = active.child.kill();
        let _ = active.child.wait();
    }
    state
        .situation
        .set_audio_state(situation::contracts::AudioState::Silent);
    Ok(())
}

#[tauri::command]
async fn meeting_preflight(
    state: tauri::State<'_, AppState>,
    input: meeting::PreflightInput,
) -> Result<meeting::PreflightResult, String> {
    let asr_health = voice::gnosis_asr::probe().await;
    let result = state.meeting.preflight(&input, asr_health)?;
    state.meeting.emit(meeting::MeetingEvent::StateChanged {
        session_id: None,
        state: result.state.clone(),
    });
    Ok(result)
}

#[tauri::command]
fn start_meeting(
    state: tauri::State<'_, AppState>,
    input: meeting::StartInput,
) -> Result<meeting::MeetingSnapshot, String> {
    start_meeting_inner(&state, &input)
}

fn start_meeting_inner(
    state: &AppState,
    input: &meeting::StartInput,
) -> Result<meeting::MeetingSnapshot, String> {
    let _policy = state
        .interaction_policy
        .lock()
        .map_err(|_| "Interaction policy lock unavailable".to_string())?;
    let tts_process = state
        .tts_process
        .lock()
        .map_err(|_| "TTS process lock unavailable".to_string())?;
    if tts_process.is_some() {
        return Err("MEETING_POLICY_TTS_BLOCKED: Stop speech and retry.".to_string());
    }
    let coding_run_active: bool = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM runtime_runs
               WHERE route_kind = 'coding.assist' AND status = 'running'
             )",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if coding_run_active {
        return Err("MEETING_POLICY_AGENT_BLOCKED: Stop the Coding Agent and retry.".to_string());
    }
    let snapshot = state.meeting.start(input, &state.connection)?;
    state.meeting.emit(meeting::MeetingEvent::StateChanged {
        session_id: snapshot.session_id.clone(),
        state: snapshot.state.clone(),
    });
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::SaaaCapturing);
    Ok(snapshot)
}

#[tauri::command]
fn get_meeting_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<meeting::MeetingSnapshot, String> {
    state.meeting.snapshot()
}

#[tauri::command]
fn watch_meeting(
    state: tauri::State<'_, AppState>,
    subscriber_id: String,
    on_event: tauri::ipc::Channel<meeting::MeetingEvent>,
) -> Result<(), String> {
    state.meeting.watch(&subscriber_id, on_event)
}

#[tauri::command]
fn unwatch_meeting(state: tauri::State<'_, AppState>, subscriber_id: String) -> Result<(), String> {
    state.meeting.unwatch(&subscriber_id)
}

#[tauri::command]
fn pause_meeting(
    state: tauri::State<'_, AppState>,
    input: meeting::SessionInput,
) -> Result<meeting::MeetingSnapshot, String> {
    let snapshot = state.meeting.pause(&input.session_id, &state.connection)?;
    state.meeting.emit(meeting::MeetingEvent::StateChanged {
        session_id: snapshot.session_id.clone(),
        state: snapshot.state.clone(),
    });
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::Inactive);
    Ok(snapshot)
}

#[tauri::command]
fn resume_meeting(
    state: tauri::State<'_, AppState>,
    input: meeting::SessionInput,
) -> Result<meeting::MeetingSnapshot, String> {
    let snapshot = state.meeting.resume(&input.session_id, &state.connection)?;
    state.meeting.emit(meeting::MeetingEvent::StateChanged {
        session_id: snapshot.session_id.clone(),
        state: snapshot.state.clone(),
    });
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::SaaaCapturing);
    Ok(snapshot)
}

#[tauri::command]
fn stop_meeting(
    state: tauri::State<'_, AppState>,
    input: meeting::SessionInput,
) -> Result<meeting::MeetingSnapshot, String> {
    let snapshot = state.meeting.stop(&input.session_id, &state.connection)?;
    state.meeting.emit(meeting::MeetingEvent::StateChanged {
        session_id: snapshot.session_id.clone(),
        state: snapshot.state.clone(),
    });
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::Inactive);
    Ok(snapshot)
}

#[tauri::command]
async fn append_meeting_audio_segment(
    state: tauri::State<'_, AppState>,
    input: meeting::SegmentInput,
) -> Result<meeting::SegmentResult, String> {
    let cancellation = Arc::new(RunCancellation::default());
    let (model, samples) = state.meeting.append(&input, cancellation.clone())?;
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::SaaaTranscribing);
    let transcription =
        voice::gnosis_asr::transcribe(&samples, input.sample_rate, &model, cancellation.clone())
            .await;
    let record_failure = |code: &str, message: String| {
        let message = redact_runtime_text(&message);
        match state
            .meeting
            .fail(&input.session_id, code, &message, &state.connection)
        {
            Ok(()) => Err(format!("{code}: {message}")),
            Err(state_error) => Err(format!("{code}: {message}; {state_error}")),
        }
    };
    let result = match transcription {
        Ok((text, language)) => {
            match state.meeting.finish_segment(&input, text, language.clone()) {
                Ok(result) => {
                    state.meeting.emit(meeting::MeetingEvent::TranscriptFinal {
                        session_id: input.session_id.clone(),
                        lane: input.lane.clone(),
                        sequence: input.sequence,
                        text: result.text.clone(),
                        language,
                    });
                    Ok(result)
                }
                Err(error) => {
                    state.meeting.abort_segment(&input);
                    if cancellation.is_cancelled() {
                        Err("Transcription cancelled".to_string())
                    } else {
                        let code = if error == "MEETING_BACKPRESSURE" {
                            "MEETING_BACKPRESSURE"
                        } else {
                            "MEETING_STT_FAILED"
                        };
                        record_failure(code, error)
                    }
                }
            }
        }
        Err(error) => {
            state.meeting.abort_segment(&input);
            if cancellation.is_cancelled() {
                Err("Transcription cancelled".to_string())
            } else {
                record_failure("MEETING_STT_FAILED", error)
            }
        }
    };
    state.situation.set_microphone_state(
        if state
            .meeting
            .snapshot()
            .is_ok_and(|snapshot| snapshot.state == meeting::MeetingState::Active)
        {
            situation::contracts::MicrophoneState::SaaaCapturing
        } else {
            situation::contracts::MicrophoneState::Inactive
        },
    );
    result
}

#[tauri::command]
async fn preview_meeting_audio_segment(
    state: tauri::State<'_, AppState>,
    input: meeting::PreviewSegmentInput,
) -> Result<(), String> {
    validate_identifier(&input.run_id, "run id")?;
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(&state, &input.run_id, cancellation.clone())?;
    let preview = state.meeting.preview(&input.segment);
    let (model, samples) = match preview {
        Ok(preview) => preview,
        Err(error) => {
            remove_active_run(&state, &input.run_id);
            return Err(error);
        }
    };
    let transcription = voice::gnosis_asr::transcribe(
        &samples,
        input.segment.sample_rate,
        &model,
        cancellation.clone(),
    )
    .await;
    remove_active_run(&state, &input.run_id);
    match transcription {
        Ok((text, language)) => {
            if state.meeting.preview_is_current(&input.segment) {
                state
                    .meeting
                    .emit(meeting::MeetingEvent::TranscriptPartial {
                        session_id: input.segment.session_id,
                        lane: input.segment.lane,
                        sequence: input.segment.sequence,
                        text,
                        language,
                    });
            }
            Ok(())
        }
        Err(_) if cancellation.is_cancelled() => Err("Meeting preview cancelled".to_string()),
        Err(error) => Err(redact_runtime_text(&error)),
    }
}

#[tauri::command]
fn save_meeting_transcript(
    state: tauri::State<'_, AppState>,
    input: meeting::SessionInput,
) -> Result<meeting::MeetingSnapshot, String> {
    let snapshot = state.meeting.save(&input.session_id, &state.connection)?;
    state.meeting.emit(meeting::MeetingEvent::StateChanged {
        session_id: snapshot.session_id.clone(),
        state: snapshot.state.clone(),
    });
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::Inactive);
    Ok(snapshot)
}

#[tauri::command]
fn discard_meeting(
    state: tauri::State<'_, AppState>,
    input: meeting::SessionInput,
) -> Result<(), String> {
    state
        .meeting
        .discard(&input.session_id, &state.connection)?;
    state.meeting.emit(meeting::MeetingEvent::StateChanged {
        session_id: None,
        state: meeting::MeetingState::Idle,
    });
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::Inactive);
    Ok(())
}

fn register_active_run(
    state: &AppState,
    run_id: &str,
    cancellation: Arc<RunCancellation>,
) -> Result<(), String> {
    if memory::control_plane::memory_enabled() {
        if let Ok(connection) = state.connection.lock() {
            let _ = memory::control_plane::cancel_running_jobs(&connection, &now_iso());
        }
    }
    let mut active = state
        .active_runs
        .lock()
        .map_err(|_| "Runtime run lock unavailable".to_string())?;
    match active.entry(run_id.to_string()) {
        Entry::Vacant(entry) => {
            entry.insert(cancellation);
        }
        Entry::Occupied(_) => return Err("A run with this id is already active".to_string()),
    }
    Ok(())
}

fn remove_active_run(state: &AppState, run_id: &str) {
    if let Ok(mut active) = state.active_runs.lock() {
        active.remove(run_id);
    }
}

fn begin_simple_runtime_run(
    state: &AppState,
    run_id: &str,
    conversation_id: &str,
    route_kind: &str,
    provider_id: &str,
) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let conversation_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
            params![conversation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !conversation_exists {
        return Err("Conversation does not exist".to_string());
    }
    connection
        .execute(
            "INSERT INTO runtime_runs(id, conversation_id, route_kind, provider_id, status, started_at)
             VALUES (?1, ?2, ?3, ?4, 'running', ?5)",
            params![run_id, conversation_id, route_kind, provider_id, now_iso()],
        )
        .map_err(database_error)?;
    Ok(())
}

fn spawn_tts_process(text: &str, voice: &str) -> Result<Child, String> {
    let mut command = match env::consts::OS {
        "macos" => {
            let mut command = Command::new("say");
            if voice != "default" {
                command.arg("-v").arg(voice);
            }
            command.arg(text);
            command
        }
        "linux" => {
            let mut command = Command::new("espeak-ng");
            if voice != "default" {
                command.arg("-v").arg(voice);
            }
            command.arg(text);
            command
        }
        "windows" => {
            let escaped = text.replace('\\', "\\\\").replace('\'', "''");
            let mut command = Command::new("powershell.exe");
            command.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("Add-Type -AssemblyName System.Speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('{escaped}')"),
            ]);
            command
        }
        _ => return Err("System TTS is not supported on this platform".to_string()),
    };
    command.stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|error| {
        format!("Could not start local system TTS: {error}. Install the platform speech runtime and retry.")
    })
}

fn validate_start_turn(input: &StartTurnInput) -> Result<(), String> {
    validate_identifier(&input.run_id, "run id")?;
    validate_identifier(&input.conversation_id, "conversation id")?;
    let content = input.content.trim();
    if content.is_empty() || content.chars().count() > 16_000 {
        return Err("Message must contain between 1 and 16,000 characters".to_string());
    }
    if let Some(workspace) = &input.workspace_path {
        if workspace.len() > 4_096 {
            return Err("Workspace path is too long".to_string());
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 160
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!("Invalid {label}"));
    }
    Ok(())
}

fn update_runtime_provider(
    state: &AppState,
    run_id: &str,
    provider_id: &str,
) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE runtime_runs SET provider_id = ?1 WHERE id = ?2 AND status = 'running'",
            params![provider_id, run_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Runtime run is not active".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderFailureKind {
    Authentication,
    Contract,
    Protocol,
    RequestTooLarge,
    Policy,
    Capacity,
    Unavailable,
    Draining,
    Upstream,
    Network,
    Timeout,
    AllocationLost,
    AllocationOutcomeUnknown,
    NotReady,
    PartialOutput,
    ClientDisconnected,
    Cancelled,
    Internal,
}

impl ProviderFailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Authentication => "authentication",
            Self::Contract => "contract",
            Self::Protocol => "protocol",
            Self::RequestTooLarge => "request-too-large",
            Self::Policy => "policy",
            Self::Capacity => "capacity",
            Self::Unavailable => "unavailable",
            Self::Draining => "draining",
            Self::Upstream => "upstream",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::AllocationLost => "allocation-lost",
            Self::AllocationOutcomeUnknown => "allocation-outcome-unknown",
            Self::NotReady => "not-ready",
            Self::PartialOutput => "partial-output",
            Self::ClientDisconnected => "client-disconnected",
            Self::Cancelled => "cancelled",
            Self::Internal => "internal",
        }
    }

    fn public_message(self) -> BoundedProviderMessage {
        let message = match self {
            Self::Authentication => {
                "Provider authentication failed. Check the configured credential."
            }
            Self::Contract => "Provider settings or request contract are invalid.",
            Self::Protocol => "Provider returned an invalid or incomplete response.",
            Self::RequestTooLarge => "Provider request or response exceeded the configured limit.",
            Self::Policy => "Provider policy rejected the request.",
            Self::Capacity => "Provider capacity is currently exhausted.",
            Self::Unavailable => "Provider is currently unavailable.",
            Self::Draining => "Provider is draining and is not accepting new work.",
            Self::Upstream => "Provider could not complete the upstream request.",
            Self::Network => "Provider connection ended before the response completed.",
            Self::Timeout => "Provider request reached its timeout.",
            Self::AllocationLost => "The selected local runtime allocation is no longer available.",
            Self::AllocationOutcomeUnknown => "The local runtime allocation outcome is unknown.",
            Self::NotReady => "The selected local runtime did not become ready in time.",
            Self::PartialOutput => "Provider output ended after a partial response.",
            Self::ClientDisconnected => "The response consumer disconnected.",
            Self::Cancelled => "Provider execution was cancelled.",
            Self::Internal => "SAAA could not complete the provider attempt.",
        };
        BoundedProviderMessage(message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BoundedProviderMessage(&'static str);

impl BoundedProviderMessage {
    fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupOutcome {
    NotApplicable,
    NotStarted,
    Released,
    DeferredToTtl {
        kind: providers::larm::contracts::ReleaseFailureKind,
    },
    GnosisDeferredToTtl {
        kind: &'static str,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum ProviderAttemptOutcome {
    Completed {
        content: String,
        cleanup: CleanupOutcome,
    },
    Cancelled {
        output_started: bool,
        cleanup: CleanupOutcome,
    },
    Failed {
        kind: ProviderFailureKind,
        public_message: BoundedProviderMessage,
        output_started: bool,
        cleanup: CleanupOutcome,
    },
}

impl ProviderAttemptOutcome {
    fn with_cleanup(self, cleanup: CleanupOutcome) -> Self {
        match self {
            Self::Completed { content, .. } => Self::Completed { content, cleanup },
            Self::Cancelled { output_started, .. } => Self::Cancelled {
                output_started,
                cleanup,
            },
            Self::Failed {
                kind,
                public_message,
                output_started,
                ..
            } => Self::Failed {
                kind,
                public_message,
                output_started,
                cleanup,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderAttemptError {
    Cancelled {
        output_started: bool,
    },
    Failed {
        kind: ProviderFailureKind,
        output_started: bool,
    },
}

impl ProviderAttemptError {
    fn failed(kind: ProviderFailureKind, output_started: bool) -> Self {
        Self::Failed {
            kind,
            output_started,
        }
    }
}

fn provider_failure_from_larm(
    kind: providers::larm::contracts::SessionFailureKind,
) -> ProviderFailureKind {
    use providers::larm::contracts::SessionFailureKind as Larm;
    match kind {
        Larm::Authentication => ProviderFailureKind::Authentication,
        Larm::Contract => ProviderFailureKind::Contract,
        Larm::Protocol => ProviderFailureKind::Protocol,
        Larm::RequestTooLarge => ProviderFailureKind::RequestTooLarge,
        Larm::Internal => ProviderFailureKind::Internal,
        Larm::ClientDisconnected => ProviderFailureKind::ClientDisconnected,
        Larm::Cancelled => ProviderFailureKind::Cancelled,
        Larm::PartialOutput => ProviderFailureKind::PartialOutput,
        Larm::Policy => ProviderFailureKind::Policy,
        Larm::Capacity => ProviderFailureKind::Capacity,
        Larm::Unavailable => ProviderFailureKind::Unavailable,
        Larm::Draining => ProviderFailureKind::Draining,
        Larm::Upstream => ProviderFailureKind::Upstream,
        Larm::Network => ProviderFailureKind::Network,
        Larm::Timeout => ProviderFailureKind::Timeout,
        Larm::AllocationLost => ProviderFailureKind::AllocationLost,
        Larm::AllocationOutcomeUnknown => ProviderFailureKind::AllocationOutcomeUnknown,
        Larm::NotReady => ProviderFailureKind::NotReady,
    }
}

fn provider_failure_from_gnosis(kind: providers::gnosis::ErrorKind) -> ProviderFailureKind {
    use providers::gnosis::ErrorKind as Gnosis;
    match kind {
        Gnosis::Authentication => ProviderFailureKind::Authentication,
        Gnosis::Contract => ProviderFailureKind::Contract,
        Gnosis::Capacity => ProviderFailureKind::Capacity,
        Gnosis::Unavailable => ProviderFailureKind::Unavailable,
        Gnosis::Upstream => ProviderFailureKind::Upstream,
        Gnosis::Network => ProviderFailureKind::Network,
        Gnosis::Timeout => ProviderFailureKind::Timeout,
        Gnosis::StaleConnection => ProviderFailureKind::AllocationLost,
        Gnosis::Cancelled => ProviderFailureKind::Cancelled,
        Gnosis::Internal => ProviderFailureKind::Internal,
    }
}

fn larm_failure_message(kind: providers::larm::contracts::SessionFailureKind) -> &'static str {
    provider_failure_from_larm(kind).public_message().as_str()
}

fn classify_reqwest_error(error: &reqwest::Error) -> ProviderFailureKind {
    if error.is_timeout() {
        ProviderFailureKind::Timeout
    } else if error.is_builder() {
        ProviderFailureKind::Internal
    } else {
        ProviderFailureKind::Network
    }
}

fn classify_provider_status(status: reqwest::StatusCode) -> ProviderFailureKind {
    match status.as_u16() {
        401 | 403 => ProviderFailureKind::Authentication,
        400 | 404 | 405 | 422 => ProviderFailureKind::Contract,
        408 | 504 => ProviderFailureKind::Timeout,
        409 | 429 => ProviderFailureKind::Capacity,
        413 => ProviderFailureKind::RequestTooLarge,
        502 => ProviderFailureKind::Upstream,
        503 => ProviderFailureKind::Unavailable,
        500..=599 => ProviderFailureKind::Upstream,
        _ => ProviderFailureKind::Protocol,
    }
}

async fn read_provider_body_limited(
    response: reqwest::Response,
    limit: usize,
    cancellation: &RunCancellation,
    output_started: bool,
) -> Result<Vec<u8>, ProviderAttemptError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let next = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderAttemptError::Cancelled { output_started }),
            next = stream.next() => next,
        };
        let Some(chunk) = next else {
            return Ok(body);
        };
        let chunk = chunk.map_err(|error| {
            ProviderAttemptError::failed(classify_reqwest_error(&error), output_started)
        })?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(ProviderAttemptError::failed(
                ProviderFailureKind::RequestTooLarge,
                output_started,
            ));
        }
        body.extend_from_slice(&chunk);
    }
}

struct ModelStreamContext<'a> {
    reasoning_effort: &'a str,
    input: &'a StartTurnInput,
    on_event: &'a tauri::ipc::Channel<RuntimeEvent>,
    cancellation: Arc<RunCancellation>,
    output_persistence: Option<ProviderOutputPersistence<'a>>,
}

async fn stream_model_provider(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    stream_model_provider_with_api_key(provider, history, timeout_ms, None, false, context).await
}

async fn stream_model_provider_with_api_key(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    api_key: Option<&str>,
    require_event_stream: bool,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    match stream_model_provider_inner(
        provider,
        history,
        timeout_ms,
        api_key,
        require_event_stream,
        context,
    )
    .await
    {
        Ok(content) => ProviderAttemptOutcome::Completed {
            content,
            cleanup: CleanupOutcome::NotApplicable,
        },
        Err(ProviderAttemptError::Cancelled { output_started }) => {
            ProviderAttemptOutcome::Cancelled {
                output_started,
                cleanup: CleanupOutcome::NotApplicable,
            }
        }
        Err(ProviderAttemptError::Failed {
            kind,
            output_started,
        }) => ProviderAttemptOutcome::Failed {
            kind,
            public_message: kind.public_message(),
            output_started,
            cleanup: CleanupOutcome::NotApplicable,
        },
    }
}

async fn stream_gnosis_provider(
    provider: &GnosisProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    let (connection, prior_cleanup) =
        match resolve_gnosis_connection_for_request(provider, timeout_ms, cancellation.clone())
            .await
        {
            Ok(connection) => connection,
            Err(failure) => {
                let kind = provider_failure_from_gnosis(failure.error.kind);
                return if kind == ProviderFailureKind::Cancelled {
                    ProviderAttemptOutcome::Cancelled {
                        output_started: false,
                        cleanup: failure.cleanup,
                    }
                } else {
                    ProviderAttemptOutcome::Failed {
                        kind,
                        public_message: kind.public_message(),
                        output_started: false,
                        cleanup: failure.cleanup,
                    }
                };
            }
        };
    let resolved = OpenAiCompatibleProviderSettings {
        id: provider.id.clone(),
        enabled: true,
        label: provider.label.clone(),
        location: "local".to_string(),
        endpoint: connection.endpoint().to_string(),
        model: connection.model().to_string(),
        credential_status: "configured".to_string(),
    };
    let outcome = stream_model_provider_with_api_key(
        &resolved,
        history,
        timeout_ms,
        Some(connection.api_key()),
        true,
        context,
    )
    .await;
    let cleanup = merge_gnosis_cleanup(
        prior_cleanup,
        gnosis_cleanup_from_release(connection.release().await),
    );
    outcome.with_cleanup(cleanup)
}

struct GnosisConnectionFailure {
    error: providers::gnosis::GnosisError,
    cleanup: CleanupOutcome,
}

fn gnosis_cleanup_from_release(
    release: Result<(), providers::gnosis::GnosisError>,
) -> CleanupOutcome {
    match release {
        Ok(()) => CleanupOutcome::Released,
        Err(error) => CleanupOutcome::GnosisDeferredToTtl {
            kind: gnosis_release_failure_kind(error.kind),
        },
    }
}

fn merge_gnosis_cleanup(previous: CleanupOutcome, current: CleanupOutcome) -> CleanupOutcome {
    match (previous, current) {
        (deferred @ CleanupOutcome::GnosisDeferredToTtl { .. }, _) => deferred,
        (_, deferred @ CleanupOutcome::GnosisDeferredToTtl { .. }) => deferred,
        (CleanupOutcome::Released, _) | (_, CleanupOutcome::Released) => CleanupOutcome::Released,
        _ => CleanupOutcome::NotStarted,
    }
}

fn gnosis_release_failure_kind(kind: providers::gnosis::ErrorKind) -> &'static str {
    use providers::gnosis::ErrorKind as Gnosis;
    match kind {
        Gnosis::Authentication => "authentication",
        Gnosis::Network => "network",
        Gnosis::Timeout => "timeout",
        Gnosis::Upstream | Gnosis::Unavailable | Gnosis::Capacity => "upstream",
        Gnosis::Contract | Gnosis::StaleConnection => "protocol",
        Gnosis::Cancelled | Gnosis::Internal => "internal",
    }
}

async fn resolve_gnosis_connection_for_request(
    provider: &GnosisProviderSettings,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<(providers::gnosis::GnosisConnection, CleanupOutcome), GnosisConnectionFailure> {
    let mut cleanup = CleanupOutcome::NotStarted;
    for attempt in 0..2 {
        let mut connection = match providers::gnosis::GnosisConnection::resolve(
            &provider.host,
            cancellation.clone(),
        )
        .await
        {
            Ok(connection) => {
                cleanup = merge_gnosis_cleanup(
                    cleanup,
                    gnosis_cleanup_from_release_failure(connection.prior_release_failure()),
                );
                connection
            }
            Err(error) => {
                cleanup = merge_gnosis_cleanup(
                    cleanup,
                    gnosis_cleanup_from_release_failure(error.release_failure()),
                );
                return Err(GnosisConnectionFailure { error, cleanup });
            }
        };
        match connection
            .ensure_lifetime(Duration::from_millis(timeout_ms), cancellation.clone())
            .await
        {
            Ok(()) => return Ok((connection, cleanup)),
            Err(error)
                if error.kind == providers::gnosis::ErrorKind::StaleConnection && attempt == 0 =>
            {
                cleanup = merge_gnosis_cleanup(
                    cleanup,
                    gnosis_cleanup_from_release(connection.release().await),
                );
            }
            Err(error) => {
                cleanup = merge_gnosis_cleanup(
                    cleanup,
                    gnosis_cleanup_from_release(connection.release().await),
                );
                return Err(GnosisConnectionFailure { error, cleanup });
            }
        }
    }
    Err(GnosisConnectionFailure {
        error: providers::gnosis::GnosisError::new(
            providers::gnosis::ErrorKind::StaleConnection,
            "The gnosis provider connection expired before inference started.",
        ),
        cleanup,
    })
}

fn gnosis_cleanup_from_release_failure(
    kind: Option<providers::gnosis::ErrorKind>,
) -> CleanupOutcome {
    kind.map_or(CleanupOutcome::NotStarted, |kind| {
        CleanupOutcome::GnosisDeferredToTtl {
            kind: gnosis_release_failure_kind(kind),
        }
    })
}

struct LarmStreamContext<'a> {
    state: &'a AppState,
    session_id: &'a str,
    input: &'a StartTurnInput,
    on_event: &'a tauri::ipc::Channel<RuntimeEvent>,
}

async fn stream_larm_provider(
    provider: &LarmProviderSettings,
    history: &[ConversationMessage],
    reasoning_effort: &str,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    context: LarmStreamContext<'_>,
) -> ProviderAttemptOutcome {
    use providers::larm::{
        client::{Cancellation, ChatMessage},
        AllocationCleanup, LarmProvider,
    };

    let larm = match LarmProvider::for_attempt(
        &context.state.larm_gate,
        &provider.base_url,
        provider.allocation_ttl_seconds,
        provider.allocation_startup_timeout_seconds,
    ) {
        Ok(larm) => larm,
        Err(kind) => {
            let kind = provider_failure_from_larm(kind);
            return ProviderAttemptOutcome::Failed {
                kind,
                public_message: kind.public_message(),
                output_started: false,
                cleanup: CleanupOutcome::NotStarted,
            };
        }
    };
    let cancellation_signal = Cancellation {
        flag: &cancellation.cancelled,
        notify: &cancellation.notify,
    };
    let mut allocation = match larm.allocate_ready(cancellation_signal).await {
        Ok(allocation) => allocation,
        Err(failure) => {
            let cleanup = match failure.cleanup {
                AllocationCleanup::NotStarted => CleanupOutcome::NotStarted,
                AllocationCleanup::Released => CleanupOutcome::Released,
                AllocationCleanup::DeferredToTtl(kind) => CleanupOutcome::DeferredToTtl { kind },
            };
            let kind = provider_failure_from_larm(failure.kind);
            if kind == ProviderFailureKind::Cancelled {
                return ProviderAttemptOutcome::Cancelled {
                    output_started: false,
                    cleanup,
                };
            }
            return ProviderAttemptOutcome::Failed {
                kind,
                public_message: kind.public_message(),
                output_started: false,
                cleanup,
            };
        }
    };

    if persist_larm_selection(context.state, context.session_id, &allocation).is_err() {
        let cleanup = cleanup_from_larm(larm.release(&allocation.allocation_id).await);
        return ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::Internal,
            public_message: ProviderFailureKind::Internal.public_message(),
            output_started: false,
            cleanup,
        };
    }
    let selection_reason_code = match allocation.selection_reason {
        providers::larm::contracts::SelectionReason::Primary => "primary",
        providers::larm::contracts::SelectionReason::Other => "other",
    };
    if context
        .on_event
        .send(RuntimeEvent::ProviderSelected {
            run_id: context.input.run_id.clone(),
            provider_id: provider.id.clone(),
            provider_kind: "larm".to_string(),
            route_id: "llm-default".to_string(),
            runtime_id: allocation.selected_runtime_id.as_str().to_string(),
            fallback_used: allocation.fallback_used,
            selection_reason_code: selection_reason_code.to_string(),
        })
        .is_err()
    {
        let _ = mark_larm_release_pending(context.state, context.session_id);
        let cleanup = cleanup_from_larm(larm.release(&allocation.allocation_id).await);
        return ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::ClientDisconnected,
            public_message: ProviderFailureKind::ClientDisconnected.public_message(),
            output_started: false,
            cleanup,
        };
    }

    let messages = history
        .iter()
        .filter_map(|message| {
            let role = match message.role.as_str() {
                "system" => "system",
                "assistant" => "assistant",
                "user" | "transcript" => "user",
                _ => return None,
            };
            Some(ChatMessage {
                role,
                content: message.content.clone(),
            })
        })
        .collect::<Vec<_>>();
    let mut tool_exchanges = Vec::<Value>::new();
    let mut tool_calls_this_attempt = 0_usize;
    let mut latest_request_id = None;
    let chat_deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let chat = loop {
        let Some(round_timeout) = chat_deadline
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
        else {
            break Err(providers::larm::client::LarmError::new(
                providers::larm::contracts::SessionFailureKind::Timeout,
                false,
            ));
        };
        let persistence = Some(ProviderOutputPersistence {
            state: context.state,
            session_id: context.session_id,
        });
        let tools = available_agent_tools(persistence, context.input, tool_calls_this_attempt);
        let round = larm
            .chat_with_tools(
                &mut allocation,
                &messages,
                &tool_exchanges,
                &tools,
                reasoning_effort,
                round_timeout,
                cancellation_signal,
                |delta, first| {
                    if first
                        && mark_provider_output_started(context.state, context.session_id).is_err()
                    {
                        return Err(providers::larm::contracts::SessionFailureKind::Internal);
                    }
                    context
                        .on_event
                        .send(RuntimeEvent::Delta {
                            run_id: context.input.run_id.clone(),
                            text: delta.to_string(),
                        })
                        .map_err(|_| {
                            providers::larm::contracts::SessionFailureKind::ClientDisconnected
                        })
                },
            )
            .await;
        match round {
            Ok(mut completion) => {
                if completion.request_id.is_some() {
                    latest_request_id = completion.request_id.clone();
                } else {
                    completion.request_id = latest_request_id.clone();
                }
                let Some(call) = completion.tool_call.clone() else {
                    break Ok(completion);
                };
                if !tool_was_offered(&tools, &call.name) {
                    break Err(providers::larm::client::LarmError::new(
                        providers::larm::contracts::SessionFailureKind::Protocol,
                        false,
                    ));
                }
                tool_calls_this_attempt += 1;
                let Some(tool_timeout) =
                    chat_deadline.checked_duration_since(tokio::time::Instant::now())
                else {
                    break Err(providers::larm::client::LarmError::new(
                        providers::larm::contracts::SessionFailureKind::Timeout,
                        false,
                    ));
                };
                let content = tokio::select! {
                    _ = cancellation.cancelled() => {
                        break Err(providers::larm::client::LarmError::new(
                            providers::larm::contracts::SessionFailureKind::Cancelled,
                            false,
                        ));
                    }
                    content = execute_agent_tool(persistence, context.input, &call, tool_timeout) => content,
                };
                runtime::agent_tools::append_tool_exchange(&mut tool_exchanges, &call, content);
            }
            Err(error) => break Err(error),
        }
    };

    let persistence_failed = match &chat {
        Ok(completion) => {
            let request_persistence_failed = persist_larm_request_id(
                context.state,
                context.session_id,
                completion.request_id.as_ref(),
            )
            .is_err();
            let release_persistence_failed =
                mark_larm_release_pending(context.state, context.session_id).is_err();
            request_persistence_failed || release_persistence_failed
        }
        Err(_) => mark_larm_release_pending(context.state, context.session_id).is_err(),
    };
    let cleanup = cleanup_from_larm(larm.release(&allocation.allocation_id).await);
    if persistence_failed {
        return ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::Internal,
            public_message: ProviderFailureKind::Internal.public_message(),
            output_started: chat
                .as_ref()
                .map(|completion| !completion.content.is_empty())
                .unwrap_or_else(|error| error.output_started),
            cleanup,
        };
    }

    match chat {
        Ok(completion) => ProviderAttemptOutcome::Completed {
            content: completion.content,
            cleanup,
        },
        Err(error) => {
            let kind = provider_failure_from_larm(error.kind);
            if kind == ProviderFailureKind::Cancelled {
                ProviderAttemptOutcome::Cancelled {
                    output_started: error.output_started,
                    cleanup,
                }
            } else {
                ProviderAttemptOutcome::Failed {
                    kind,
                    public_message: kind.public_message(),
                    output_started: error.output_started,
                    cleanup,
                }
            }
        }
    }
}

fn cleanup_from_larm(cleanup: providers::larm::client::CleanupResult) -> CleanupOutcome {
    match cleanup {
        providers::larm::client::CleanupResult::Released => CleanupOutcome::Released,
        providers::larm::client::CleanupResult::DeferredToTtl(kind) => {
            CleanupOutcome::DeferredToTtl { kind }
        }
    }
}

async fn stream_model_provider_inner(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    api_key: Option<&str>,
    require_event_stream: bool,
    context: ModelStreamContext<'_>,
) -> Result<String, ProviderAttemptError> {
    if context.cancellation.is_cancelled() {
        return Err(ProviderAttemptError::Cancelled {
            output_started: false,
        });
    }
    let mut client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .redirect(reqwest::redirect::Policy::none());
    if provider.location == "local" {
        client = client.no_proxy();
    }
    let client = client
        .build()
        .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Internal, false))?;
    let mut messages = history
        .iter()
        .filter_map(|message| {
            let role = match message.role.as_str() {
                "system" => "system",
                "assistant" => "assistant",
                "user" | "transcript" => "user",
                _ => return None,
            };
            Some(json!({ "role": role, "content": message.content }))
        })
        .collect::<Vec<_>>();
    let mut tool_calls_this_attempt = 0_usize;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let round_timeout = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| ProviderAttemptError::failed(ProviderFailureKind::Timeout, false))?;
        let tools = available_agent_tools(
            context.output_persistence,
            context.input,
            tool_calls_this_attempt,
        );
        match stream_model_provider_round(
            &client,
            provider,
            &messages,
            &tools,
            context.reasoning_effort,
            api_key,
            context.input,
            context.on_event,
            context.cancellation.clone(),
            context.output_persistence,
            round_timeout,
            require_event_stream,
        )
        .await?
        {
            ModelProviderCompletion::Content(content) => return Ok(content),
            ModelProviderCompletion::ToolCall(call) => {
                if !tool_was_offered(&tools, &call.name) {
                    return Err(ProviderAttemptError::failed(
                        ProviderFailureKind::Protocol,
                        false,
                    ));
                }
                tool_calls_this_attempt += 1;
                let tool_timeout = deadline
                    .checked_duration_since(tokio::time::Instant::now())
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or_else(|| {
                        ProviderAttemptError::failed(ProviderFailureKind::Timeout, false)
                    })?;
                let tool_content = tokio::select! {
                    _ = context.cancellation.cancelled() => {
                        return Err(ProviderAttemptError::Cancelled { output_started: false });
                    }
                    content = execute_agent_tool(
                        context.output_persistence,
                        context.input,
                        &call,
                        tool_timeout,
                    ) => content,
                };
                runtime::agent_tools::append_tool_exchange(&mut messages, &call, tool_content);
            }
        }
    }
}

enum ModelProviderCompletion {
    Content(String),
    ToolCall(runtime::agent_tools::AgentToolCall),
}

#[allow(clippy::too_many_arguments)]
async fn stream_model_provider_round(
    client: &reqwest::Client,
    provider: &OpenAiCompatibleProviderSettings,
    messages: &[Value],
    tools: &[Value],
    reasoning_effort: &str,
    api_key: Option<&str>,
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: Arc<RunCancellation>,
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    round_timeout: Duration,
    require_event_stream: bool,
) -> Result<ModelProviderCompletion, ProviderAttemptError> {
    let mut body = json!({
        "model": provider.model,
        "messages": messages,
        "stream": true,
        "reasoning_effort": reasoning_effort
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = json!("auto");
    }
    let mut request = client
        .post(
            provider_chat_url(&provider.endpoint)
                .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Contract, false))?,
        )
        .timeout(round_timeout)
        .json(&body);
    let configured_api_key = provider_api_key(provider);
    if let Some(api_key) = api_key.or(configured_api_key.as_deref()) {
        request = request.bearer_auth(api_key);
    }
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(ProviderAttemptError::Cancelled { output_started: false }),
        response = request.send() => response.map_err(|error| {
            ProviderAttemptError::failed(classify_reqwest_error(&error), false)
        })?,
    };
    if !response.status().is_success() {
        return Err(ProviderAttemptError::failed(
            classify_provider_status(response.status()),
            false,
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if require_event_stream && !content_type.starts_with("text/event-stream") {
        return Err(ProviderAttemptError::failed(
            ProviderFailureKind::Protocol,
            false,
        ));
    }
    if content_type.contains("application/json") {
        let body = read_provider_body_limited(response, 1_048_576, &cancellation, false).await?;
        let response: Value = serde_json::from_slice(&body)
            .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Protocol, false))?;
        let tool_call = runtime::agent_tools::parse_non_stream_tool_call(&response)
            .map_err(|error| tool_protocol_failure(error, false))?;
        let content = response
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str);
        if let Some(call) = tool_call {
            if content.is_some_and(|value| !value.trim().is_empty()) {
                return Err(ProviderAttemptError::failed(
                    ProviderFailureKind::Protocol,
                    false,
                ));
            }
            return Ok(ModelProviderCompletion::ToolCall(call));
        }
        let content = content
            .ok_or_else(|| ProviderAttemptError::failed(ProviderFailureKind::Protocol, false))?;
        if content.chars().count() > 64_000 {
            return Err(ProviderAttemptError::failed(
                ProviderFailureKind::RequestTooLarge,
                false,
            ));
        }
        let content = content.to_string();
        if content.trim().is_empty() {
            return Err(ProviderAttemptError::failed(
                ProviderFailureKind::Protocol,
                false,
            ));
        }
        if let Some(persistence) = output_persistence {
            persistence.mark_started()?;
        }
        on_event
            .send(RuntimeEvent::Delta {
                run_id: input.run_id.clone(),
                text: content.clone(),
            })
            .map_err(|_| {
                ProviderAttemptError::failed(ProviderFailureKind::ClientDisconnected, true)
            })?;
        return Ok(ModelProviderCompletion::Content(content));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut content = String::new();
    let mut content_chars = 0_usize;
    let mut output_started = false;
    let mut stream_completed = false;
    let mut tool_calls = runtime::agent_tools::ToolCallAccumulator::default();
    loop {
        if cancellation.is_cancelled() {
            return Err(ProviderAttemptError::Cancelled { output_started });
        }
        let next = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderAttemptError::Cancelled { output_started }),
            next = stream.next() => next,
        };
        let Some(chunk) = next else {
            if !buffer.is_empty() {
                buffer.extend_from_slice(b"\n\n");
                stream_completed = project_sse_events(
                    drain_sse_events(&mut buffer, 1_048_576)
                        .map_err(|error| sse_drain_failure(error, output_started))?,
                    &mut content,
                    &mut content_chars,
                    &mut output_started,
                    input,
                    on_event,
                    output_persistence,
                    &mut tool_calls,
                )?;
            }
            break;
        };
        let chunk = chunk.map_err(|error| {
            ProviderAttemptError::failed(classify_reqwest_error(&error), output_started)
        })?;
        buffer.extend_from_slice(&chunk);
        let events = drain_sse_events(&mut buffer, 1_048_576)
            .map_err(|error| sse_drain_failure(error, output_started))?;
        if buffer.len() > 1_048_576 {
            return Err(ProviderAttemptError::failed(
                ProviderFailureKind::RequestTooLarge,
                output_started,
            ));
        }
        let stream_done = project_sse_events(
            events,
            &mut content,
            &mut content_chars,
            &mut output_started,
            input,
            on_event,
            output_persistence,
            &mut tool_calls,
        )?;
        if stream_done {
            stream_completed = true;
            break;
        }
    }
    if !stream_completed {
        return Err(ProviderAttemptError::failed(
            ProviderFailureKind::Network,
            output_started,
        ));
    }
    let tool_call = tool_calls
        .finish()
        .map_err(|error| tool_protocol_failure(error, output_started))?;
    if let Some(call) = tool_call {
        if !content.trim().is_empty() {
            return Err(ProviderAttemptError::failed(
                ProviderFailureKind::Protocol,
                output_started,
            ));
        }
        return Ok(ModelProviderCompletion::ToolCall(call));
    }
    if content.trim().is_empty() {
        return Err(ProviderAttemptError::failed(
            ProviderFailureKind::Protocol,
            false,
        ));
    }
    Ok(ModelProviderCompletion::Content(content))
}

fn available_agent_tools(
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    input: &StartTurnInput,
    calls_this_attempt: usize,
) -> Vec<Value> {
    if calls_this_attempt >= memory::contracts::MAX_RECALL_CALLS_PER_TURN {
        return Vec::new();
    }
    let include_conversation = output_persistence.is_some_and(|persistence| {
        persistence
            .state
            .connection
            .lock()
            .ok()
            .and_then(|connection| memory::recall::remaining_calls(&connection, &input.run_id).ok())
            .is_some_and(|remaining| remaining > 0)
    });
    let include_typed_memory = output_persistence
        .is_some_and(|persistence| persistence.state.context_still_recall.is_configured());
    runtime::agent_tools::agent_tool_definitions(include_conversation, include_typed_memory)
}

fn tool_was_offered(definitions: &[Value], name: &str) -> bool {
    definitions.iter().any(|definition| {
        definition.pointer("/function/name").and_then(Value::as_str) == Some(name)
    })
}

async fn execute_agent_tool(
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    input: &StartTurnInput,
    call: &runtime::agent_tools::AgentToolCall,
    timeout: Duration,
) -> String {
    if runtime::agent_tools::is_typed_memory_tool(&call.name) {
        let Some(persistence) = output_persistence else {
            return runtime::agent_tools::tool_error_content(
                "typed-memory-unavailable",
                "Typed memory recall is temporarily unavailable.",
            );
        };
        return match tokio::time::timeout(
            timeout,
            persistence
                .state
                .context_still_recall
                .recall(&call.name, &call.arguments),
        )
        .await
        {
            Ok(Ok(content)) => content,
            Ok(Err(error)) => {
                runtime::agent_tools::tool_error_content(error.tool_code(), error.safe_message())
            }
            Err(_) => runtime::agent_tools::tool_error_content(
                "typed-memory-unavailable",
                "Typed memory recall is temporarily unavailable.",
            ),
        };
    }
    execute_recall_tool(output_persistence, input, call)
}

fn execute_recall_tool(
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    input: &StartTurnInput,
    call: &runtime::agent_tools::AgentToolCall,
) -> String {
    let Some(persistence) = output_persistence else {
        return runtime::agent_tools::tool_error_content(
            "local-recall-unavailable",
            "Local conversation recall is unavailable for this request.",
        );
    };
    let mut connection = match persistence.state.connection.lock() {
        Ok(connection) => connection,
        Err(_) => {
            return runtime::agent_tools::tool_error_content(
                "local-recall-unavailable",
                "Local conversation recall is temporarily unavailable.",
            );
        }
    };
    let context = memory::recall::RecallExecutionContext {
        runtime_run_id: &input.run_id,
        tool_call_id: &call.id,
        now: chrono::Utc::now(),
        timezone: memory::recall::system_timezone(),
    };
    let arguments = match runtime::agent_tools::parse_recall_arguments(&call.arguments) {
        Ok(arguments) => arguments,
        Err(()) => {
            return match memory::recall::record_failed_attempt(&mut connection, &context) {
                Ok(()) => runtime::agent_tools::tool_error_content(
                    "invalid-input",
                    "Tool arguments do not match the recall_conversation schema.",
                ),
                Err(error) => {
                    runtime::agent_tools::tool_error_content(error.code.as_str(), error.message)
                }
            };
        }
    };
    match memory::recall::execute(&mut connection, context, arguments) {
        Ok(output) => serde_json::to_string(&output).unwrap_or_else(|_| {
            runtime::agent_tools::tool_error_content(
                "local-recall-unavailable",
                "The conversation recall result could not be encoded.",
            )
        }),
        Err(error) => runtime::agent_tools::tool_error_content(error.code.as_str(), error.message),
    }
}

fn tool_protocol_failure(
    error: runtime::agent_tools::ToolProtocolError,
    output_started: bool,
) -> ProviderAttemptError {
    let kind = match error {
        runtime::agent_tools::ToolProtocolError::Protocol => ProviderFailureKind::Protocol,
        runtime::agent_tools::ToolProtocolError::TooLarge => ProviderFailureKind::RequestTooLarge,
    };
    ProviderAttemptError::failed(kind, output_started)
}

fn sse_drain_failure(error: SseDrainError, output_started: bool) -> ProviderAttemptError {
    let kind = match error {
        SseDrainError::InvalidUtf8 => ProviderFailureKind::Protocol,
        SseDrainError::EventTooLarge => ProviderFailureKind::RequestTooLarge,
    };
    ProviderAttemptError::failed(kind, output_started)
}

#[allow(clippy::too_many_arguments)]
fn project_sse_events(
    events: Vec<String>,
    content: &mut String,
    content_chars: &mut usize,
    output_started: &mut bool,
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    tool_calls: &mut runtime::agent_tools::ToolCallAccumulator,
) -> Result<bool, ProviderAttemptError> {
    for event in events {
        for line in event.lines().filter_map(|line| line.strip_prefix("data:")) {
            let data = line.trim();
            if data == "[DONE]" {
                return Ok(true);
            }
            let value: Value = serde_json::from_str(data).map_err(|_| {
                ProviderAttemptError::failed(ProviderFailureKind::Protocol, *output_started)
            })?;
            tool_calls
                .absorb_stream_delta(&value)
                .map_err(|error| tool_protocol_failure(error, *output_started))?;
            if let Some(delta) = value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
            {
                let delta_chars = delta.chars().count();
                if delta_chars == 0 {
                    continue;
                }
                let remaining = 64_000usize.saturating_sub(*content_chars);
                if remaining == 0 || delta_chars > remaining {
                    return Err(ProviderAttemptError::failed(
                        ProviderFailureKind::RequestTooLarge,
                        *output_started,
                    ));
                }
                if !*output_started {
                    if let Some(persistence) = output_persistence {
                        persistence.mark_started()?;
                    }
                }
                content.push_str(delta);
                *content_chars += delta_chars;
                *output_started = true;
                on_event
                    .send(RuntimeEvent::Delta {
                        run_id: input.run_id.clone(),
                        text: delta.to_string(),
                    })
                    .map_err(|_| {
                        ProviderAttemptError::failed(ProviderFailureKind::ClientDisconnected, true)
                    })?;
            }
        }
    }
    Ok(false)
}

#[derive(Clone, Copy)]
struct ProviderOutputPersistence<'a> {
    state: &'a AppState,
    session_id: &'a str,
}

impl ProviderOutputPersistence<'_> {
    fn mark_started(self) -> Result<(), ProviderAttemptError> {
        mark_provider_output_started(self.state, self.session_id)
            .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Internal, false))
    }
}

#[tauri::command]
async fn list_codex_models() -> Result<Vec<CodexModelOption>, String> {
    match tauri::async_runtime::spawn_blocking(fetch_codex_models).await {
        Ok(result) => result.map_err(|error| redact_runtime_text(&error)),
        Err(error) => Err(redact_runtime_text(&format!(
            "Codex model lookup task failed: {error}"
        ))),
    }
}

#[tauri::command]
async fn get_codex_status() -> CodexRuntimeStatus {
    match tauri::async_runtime::spawn_blocking(fetch_codex_status).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => CodexRuntimeStatus {
            installed: false,
            authenticated: false,
            runtime: "unavailable".to_string(),
            account_type: None,
            message: redact_runtime_text(&error),
        },
        Err(error) => CodexRuntimeStatus {
            installed: false,
            authenticated: false,
            runtime: "unavailable".to_string(),
            account_type: None,
            message: redact_runtime_text(&format!("Codex health check failed: {error}")),
        },
    }
}

fn spawn_bounded_codex_reader<R>(
    stdout: R,
) -> (
    mpsc::Receiver<Result<Value, String>>,
    thread::JoinHandle<()>,
)
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(256);
    let reader = thread::spawn(move || {
        let mut stdout = BufReader::new(stdout.take(MAX_CODEX_STDOUT_BYTES + 1));
        let mut bytes_read = 0_u64;
        loop {
            let mut line = String::new();
            let count = match stdout.read_line(&mut line) {
                Ok(0) => break,
                Ok(count) => count,
                Err(error) => {
                    let _ = sender.send(Err(format!(
                        "Could not read Codex app-server response: {error}"
                    )));
                    break;
                }
            };
            bytes_read = bytes_read.saturating_add(count as u64);
            if bytes_read > MAX_CODEX_STDOUT_BYTES {
                let _ = sender.send(Err(
                    "Codex app-server output exceeded the 4 MiB limit".to_string()
                ));
                break;
            }
            let message = serde_json::from_str::<Value>(line.trim_end())
                .map_err(|error| format!("Codex app-server returned invalid JSON: {error}"));
            if sender.send(message).is_err() {
                break;
            }
        }
    });
    (receiver, reader)
}

fn fetch_codex_status() -> Result<CodexRuntimeStatus, String> {
    let mut child = ProcessGuard::new(spawn_codex_app_server()?);
    let mut stdin = child
        .child_mut()
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server stdin is unavailable".to_string())?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server stdout is unavailable".to_string())?;
    let (receiver, stdout_reader) = spawn_bounded_codex_reader(stdout);
    let result = (|| {
        write_codex_handshake(&mut stdin)?;
        write_codex_message(
            &mut stdin,
            json!({ "method": "account/read", "id": 2, "params": { "refreshToken": false } }),
        )?;
        let response = receive_codex_response(&receiver, 2, Duration::from_secs(15))?;
        let account = response.pointer("/result/account");
        let account_type = account
            .and_then(|account| account.get("type"))
            .and_then(Value::as_str)
            .filter(|value| value.chars().count() <= 80 && !value.chars().any(char::is_control))
            .map(str::to_string);
        let authenticated = account.is_some_and(|value| !value.is_null());
        Ok(CodexRuntimeStatus {
            installed: true,
            authenticated,
            runtime: "app-server".to_string(),
            account_type,
            message: if authenticated {
                "Codex is ready".to_string()
            } else {
                "Codex is installed but not authenticated. Run codex login.".to_string()
            },
        })
    })();
    drop(stdin);
    drop(receiver);
    child.terminate();
    if stdout_reader.join().is_err() && result.is_ok() {
        return Err("Codex output reader stopped unexpectedly".to_string());
    }
    result
}

fn fetch_codex_models() -> Result<Vec<CodexModelOption>, String> {
    let mut child = ProcessGuard::new(spawn_codex_app_server()?);
    let mut stdin = child
        .child_mut()
        .stdin
        .take()
        .ok_or_else(|| "Codex app-server stdin is unavailable".to_string())?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| "Codex app-server stdout is unavailable".to_string())?;
    let (receiver, stdout_reader) = spawn_bounded_codex_reader(stdout);

    let result = (|| {
        write_codex_message(
            &mut stdin,
            json!({
                "method": "initialize",
                "id": 1,
                "params": {
                    "clientInfo": {
                        "name": "saaa",
                        "title": "SAAA",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            }),
        )?;
        write_codex_message(&mut stdin, json!({ "method": "initialized", "params": {} }))?;

        let mut request_id = 2_u64;
        let mut cursor: Option<String> = None;
        let mut seen_cursors = std::collections::HashSet::new();
        let mut seen_model_ids = std::collections::HashSet::new();
        let mut models = Vec::new();
        let mut page_count = 0_usize;
        let lookup_deadline = std::time::Instant::now() + Duration::from_secs(60);

        loop {
            if page_count >= 20 {
                return Err("Codex model pagination exceeded the 20-page limit".to_string());
            }
            page_count += 1;
            let mut params = json!({ "limit": 100, "includeHidden": false });
            if let Some(value) = &cursor {
                params["cursor"] = Value::String(value.clone());
            }
            write_codex_message(
                &mut stdin,
                json!({ "method": "model/list", "id": request_id, "params": params }),
            )?;

            let page_deadline =
                (std::time::Instant::now() + Duration::from_secs(20)).min(lookup_deadline);
            let page = loop {
                let remaining = page_deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err("Timed out while loading models from Codex".to_string());
                }
                let message = receiver
                    .recv_timeout(remaining)
                    .map_err(|error| match error {
                        mpsc::RecvTimeoutError::Timeout => {
                            "Timed out while loading models from Codex".to_string()
                        }
                        mpsc::RecvTimeoutError::Disconnected => {
                            "Codex app-server stopped before returning models".to_string()
                        }
                    })??;
                if message.get("id").and_then(Value::as_u64) != Some(request_id) {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    let detail = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown Codex app-server error");
                    return Err(format!(
                        "Could not load Codex models: {}",
                        redact_runtime_text(detail)
                    ));
                }
                let result = message
                    .get("result")
                    .cloned()
                    .ok_or_else(|| "Codex model response did not include a result".to_string())?;
                let page = serde_json::from_value::<CodexModelPage>(result)
                    .map_err(|error| format!("Could not decode Codex models: {error}"))?;
                for model in &page.data {
                    validate_codex_model_option(model)?;
                    if !seen_model_ids.insert(model.id.clone()) {
                        return Err("Codex model list contained a duplicate id".to_string());
                    }
                }
                break page;
            };

            if models.len().saturating_add(page.data.len()) > 2_000 {
                return Err("Codex model list exceeded the 2,000-item limit".to_string());
            }
            models.extend(page.data.into_iter().filter(|model| !model.hidden));
            cursor = match page.next_cursor {
                Some(next) if next.is_empty() || next.len() > 1_024 => {
                    return Err("Codex model cursor is invalid".to_string())
                }
                Some(next) if !seen_cursors.insert(next.clone()) => {
                    return Err("Codex model pagination repeated a cursor".to_string())
                }
                next => next,
            };
            if cursor.is_none() {
                break;
            }
            request_id = request_id
                .checked_add(1)
                .ok_or_else(|| "Codex model request id overflowed".to_string())?;
        }

        Ok(models)
    })();

    drop(stdin);
    drop(receiver);
    child.terminate();
    if stdout_reader.join().is_err() && result.is_ok() {
        return Err("Codex output reader stopped unexpectedly".to_string());
    }
    result
}

fn validate_codex_model_option(model: &CodexModelOption) -> Result<(), String> {
    fn valid(value: &str, max_chars: usize, allow_empty: bool) -> bool {
        (allow_empty || !value.is_empty())
            && value.chars().count() <= max_chars
            && !value.chars().any(char::is_control)
    }

    if !valid(&model.id, 160, false)
        || !valid(&model.model, 160, false)
        || !valid(&model.display_name, 200, true)
        || !valid(&model.description, 2_000, true)
        || model
            .default_reasoning_effort
            .as_deref()
            .is_some_and(|value| !valid(value, 80, false))
        || model.supported_reasoning_efforts.len() > 16
        || model.supported_reasoning_efforts.iter().any(|effort| {
            !valid(&effort.reasoning_effort, 80, false) || !valid(&effort.description, 500, true)
        })
        || model.input_modalities.len() > 16
        || model
            .input_modalities
            .iter()
            .any(|modality| !valid(modality, 80, false))
    {
        return Err("Codex model response contained invalid bounded fields".to_string());
    }
    Ok(())
}

fn write_codex_message(stdin: &mut impl Write, message: Value) -> Result<(), String> {
    writeln!(stdin, "{message}")
        .and_then(|_| stdin.flush())
        .map_err(|error| format!("Could not write to Codex app-server: {error}"))
}

fn write_codex_handshake(stdin: &mut impl Write) -> Result<(), String> {
    write_codex_message(
        stdin,
        json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "saaa",
                    "title": "SAAA",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": null
            }
        }),
    )?;
    write_codex_message(stdin, json!({ "method": "initialized", "params": {} }))
}

fn receive_codex_response(
    receiver: &mpsc::Receiver<Result<Value, String>>,
    request_id: u64,
    timeout: Duration,
) -> Result<Value, String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err("Timed out waiting for Codex app-server".to_string());
        }
        let message = receiver
            .recv_timeout(remaining)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => {
                    "Timed out waiting for Codex app-server".to_string()
                }
                mpsc::RecvTimeoutError::Disconnected => {
                    "Codex app-server stopped before responding".to_string()
                }
            })??;
        if message.get("id").and_then(Value::as_u64) != Some(request_id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            return Err(format!(
                "Codex app-server error: {}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            ));
        }
        return Ok(message);
    }
}

fn spawn_codex_app_server() -> Result<Child, String> {
    let mut errors = Vec::new();
    for executable in codex_executable_candidates() {
        if executable.is_absolute() && !executable.exists() {
            continue;
        }
        let mut command = Command::new(&executable);
        configure_codex_environment(&mut command);
        match command
            .args([
                "--config",
                "mcp_servers={}",
                "--config",
                "web_search=\"disabled\"",
                "--config",
                "sandbox_workspace_write.network_access=false",
                "app-server",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => return Ok(child),
            Err(error) => errors.push(format!("{}: {error}", executable.display())),
        }
    }
    Err(format!(
        "Could not start the Codex runtime. Install @openai/codex-sdk or set SAAA_CODEX_PATH. {}",
        errors.join("; ")
    ))
}

fn configure_codex_environment(command: &mut Command) {
    command.env_clear();
    for key in [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "TMPDIR",
        "TEMP",
        "TMP",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "CODEX_HOME",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "NO_PROXY",
    ] {
        if let Some(value) = env::var_os(key) {
            command.env(key, value);
        }
    }
}

fn codex_executable_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("SAAA_CODEX_PATH").filter(|value| !value.is_empty()) {
        candidates.push(PathBuf::from(path));
    }
    if let Some(path) = BUNDLED_CODEX_PATH.get() {
        candidates.push(path.clone());
    }

    if let Some((package, target, executable)) = codex_platform_package() {
        candidates.push(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("node_modules")
                .join("@openai")
                .join(package)
                .join("vendor")
                .join(target)
                .join("bin")
                .join(executable),
        );
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("node_modules")
            .join(".bin")
            .join(if cfg!(windows) { "codex.cmd" } else { "codex" }),
    );
    candidates.push(PathBuf::from("codex"));
    candidates
}

fn codex_platform_package() -> Option<(&'static str, &'static str, &'static str)> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Some(("codex-darwin-arm64", "aarch64-apple-darwin", "codex")),
        ("macos", "x86_64") => Some(("codex-darwin-x64", "x86_64-apple-darwin", "codex")),
        ("linux", "aarch64") => Some(("codex-linux-arm64", "aarch64-unknown-linux-musl", "codex")),
        ("linux", "x86_64") => Some(("codex-linux-x64", "x86_64-unknown-linux-musl", "codex")),
        ("windows", "aarch64") => {
            Some(("codex-win32-arm64", "aarch64-pc-windows-msvc", "codex.exe"))
        }
        ("windows", "x86_64") => Some(("codex-win32-x64", "x86_64-pc-windows-msvc", "codex.exe")),
        _ => None,
    }
}

fn default_codex_input_modalities() -> Vec<String> {
    vec!["text".to_string(), "image".to_string()]
}

#[tauri::command]
fn save_settings_documents(
    state: tauri::State<'_, AppState>,
    input: SaveSettingsDocumentsInput,
) -> Result<Vec<SettingsDocument>, String> {
    for document in &input.documents {
        validate_settings_document(document)?;
    }
    validate_settings_batch(&input.documents)?;
    let situation_settings = input
        .documents
        .iter()
        .find(|document| document.namespace == "situation.runtime" && document.key == "default")
        .ok_or_else(|| "Situation settings are required".to_string())
        .and_then(|document| {
            serde_json::from_value::<situation::contracts::SituationRuntimeSettings>(
                document.value_json.clone(),
            )
            .map_err(|error| format!("Invalid Situation settings: {error}"))
        })?;
    let enabled = situation_settings.enabled;
    let saved = state.situation.configure_and_persist(
        &state.connection,
        situation_settings,
        |connection| save_settings_documents_to_connection(connection, &input.documents),
    )?;
    if enabled {
        spawn_situation_monitor(state.connection.clone(), state.situation.clone());
    }
    Ok(saved)
}

#[tauri::command]
fn create_conversation(
    state: tauri::State<'_, AppState>,
    input: CreateConversationInput,
) -> Result<Conversation, String> {
    if !matches!(input.task_mode.as_str(), "conversation" | "coding") {
        return Err("Unsupported task mode".to_string());
    }
    let title = input
        .title
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty());
    if title
        .as_ref()
        .is_some_and(|title| title.chars().count() > 120)
    {
        return Err("Conversation title exceeds the 120 character limit".to_string());
    }
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    if input.task_mode == "conversation" {
        return ensure_primary_conversation(&connection);
    }
    let now = now_iso();
    let conversation = Conversation {
        id: new_id("conversation"),
        title,
        task_mode: input.task_mode,
        created_at: now.clone(),
        updated_at: now,
    };
    connection
        .execute(
            "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                conversation.id,
                conversation.title,
                conversation.task_mode,
                conversation.created_at,
                conversation.updated_at,
            ],
        )
        .map_err(database_error)?;
    Ok(conversation)
}

#[tauri::command]
fn list_messages(
    state: tauri::State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<ConversationMessage>, String> {
    validate_identifier(&conversation_id, "conversation id")?;
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    list_messages_from_connection(&connection, &conversation_id)
}

#[tauri::command]
fn append_message(
    state: tauri::State<'_, AppState>,
    input: AppendMessageInput,
) -> Result<ConversationMessage, String> {
    validate_identifier(&input.conversation_id, "conversation id")?;
    let content = input.content.trim();
    if content.is_empty() {
        return Err("Message cannot be empty".to_string());
    }
    if content.chars().count() > 16_000 {
        return Err("Message exceeds the 16,000 character limit".to_string());
    }
    if !matches!(
        input.role.as_str(),
        "user" | "assistant" | "system" | "transcript"
    ) {
        return Err("Unsupported message role".to_string());
    }

    let message = ConversationMessage {
        id: new_id("message"),
        conversation_id: input.conversation_id,
        role: input.role,
        content: content.to_string(),
        created_at: now_iso(),
    };
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let transaction = connection.transaction().map_err(database_error)?;
    let task_mode: String = transaction
        .query_row(
            "SELECT task_mode FROM conversations WHERE id = ?1",
            params![message.conversation_id],
            |row| row.get(0),
        )
        .map_err(|_| "Conversation does not exist".to_string())?;
    validate_conversation_write_target(&message.conversation_id, &task_mode)?;
    transaction
        .execute(
            "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.id,
                message.conversation_id,
                message.role,
                message.content,
                message.created_at,
            ],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![message.created_at, message.conversation_id],
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)?;
    Ok(message)
}

fn application_database_path(app: &tauri::App) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if env::var_os("SAAA_SMOKE_MARKER_ID").is_some() {
        if let Some(directory) = env::var_os("SAAA_SMOKE_DATA_DIR").map(PathBuf::from) {
            if !directory.is_absolute() {
                return Err("SAAA_SMOKE_DATA_DIR must be absolute".into());
            }
            fs::create_dir_all(&directory)?;
            return Ok(directory.join("saaa.sqlite3"));
        }
    }
    let directory = app.path().app_data_dir()?;
    if let Some(readiness_directory) = env::var_os("SAAA_MVP2X_APP_DATA_DIR").map(PathBuf::from) {
        let readiness_directory =
            validate_readiness_data_directory(&readiness_directory, &directory)
                .map_err(std::io::Error::other)?;
        return Ok(readiness_directory.join("saaa.sqlite3"));
    }
    fs::create_dir_all(&directory)?;
    Ok(directory.join("saaa.sqlite3"))
}

fn validate_readiness_data_directory(
    directory: &Path,
    normal_app_data: &Path,
) -> Result<PathBuf, String> {
    if !directory.is_absolute() {
        return Err("SAAA_MVP2X_APP_DATA_DIR must be absolute".to_string());
    }
    let metadata = fs::symlink_metadata(directory)
        .map_err(|_| "SAAA_MVP2X_APP_DATA_DIR must be an existing directory".to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("SAAA_MVP2X_APP_DATA_DIR must be a real directory".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err("SAAA_MVP2X_APP_DATA_DIR must have mode 0700".to_string());
        }
    }
    let canonical = fs::canonicalize(directory)
        .map_err(|_| "SAAA_MVP2X_APP_DATA_DIR could not be resolved".to_string())?;
    let normal =
        fs::canonicalize(normal_app_data).unwrap_or_else(|_| normal_app_data.to_path_buf());
    if canonical == normal {
        return Err("SAAA_MVP2X_APP_DATA_DIR must not use normal application data".to_string());
    }
    Ok(canonical)
}

fn now_iso() -> String {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{milliseconds}")
}

fn new_id(prefix: &str) -> String {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let nanoseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{nanoseconds}_{sequence}")
}

fn database_error(error: rusqlite::Error) -> String {
    format!("SQLite operation failed: {error}")
}

fn shutdown_app_state(state: &AppState) {
    if let Ok(mut process) = state.tts_process.lock() {
        if let Some(mut active) = process.take() {
            let _ = active.child.kill();
            let _ = active.child.wait();
        }
    }
    if let Ok(active_runs) = state.active_runs.lock() {
        for cancellation in active_runs.values() {
            cancellation.cancel();
        }
    }
    state.meeting.shutdown(&state.connection);
    let _ = state.situation.flush_quality(&state.connection);
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::Inactive);
    state
        .situation
        .set_audio_state(situation::contracts::AudioState::Silent);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let database_path = application_database_path(app)?;
            let voice_resource_directory = app
                .path()
                .resolve("voice", tauri::path::BaseDirectory::Resource)?;
            let voice_data_directory = database_path
                .parent()
                .ok_or_else(|| std::io::Error::other("Database path has no parent directory"))?
                .to_path_buf();
            let voice_profile = Arc::new(voice::profile::VoiceProfileRuntime::initialize(
                voice_resource_directory,
                voice_data_directory.clone(),
            ));
            let bundled_codex = app.path().resolve(
                if cfg!(windows) {
                    "bin/codex.exe"
                } else {
                    "bin/codex"
                },
                tauri::path::BaseDirectory::Resource,
            )?;
            if bundled_codex.is_file() {
                let _ = BUNDLED_CODEX_PATH.set(bundled_codex);
            }
            let connection = Connection::open(&database_path)?;
            backup_before_migration(&connection, &database_path).map_err(std::io::Error::other)?;
            initialize_database(&connection)?;
            let situation_settings =
                situation::repository::load_settings(&connection).map_err(std::io::Error::other)?;
            let latest_situation =
                situation::repository::latest_entry(&connection).map_err(std::io::Error::other)?;
            let active_profile = situation::calibration::active_profile(&connection)
                .map_err(std::io::Error::other)?;
            let connection = Arc::new(Mutex::new(connection));
            let situation = Arc::new(
                situation::SituationRuntime::new(
                    situation_settings.clone(),
                    latest_situation.as_ref(),
                )
                .map_err(std::io::Error::other)?,
            );
            situation
                .set_calibration_profile(active_profile)
                .map_err(std::io::Error::other)?;
            if situation_settings.enabled {
                spawn_situation_monitor(connection.clone(), situation.clone());
            }
            app.manage(AppState {
                connection,
                data_directory: voice_data_directory,
                context_still_recall:
                    memory::context_still_recall::ContextStillRecallClient::from_environment(),
                active_runs: Mutex::new(HashMap::new()),
                interaction_policy: Mutex::new(()),
                shutdown_started: AtomicBool::new(false),
                larm_gate: providers::larm::LarmRuntimeGate::initialize(),
                tts_process: Mutex::new(None),
                situation,
                meeting: Arc::new(meeting::MeetingRuntime::new()),
                voice_profile,
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            let tauri::WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            let state = window.state::<AppState>();
            if state.shutdown_started.swap(true, Ordering::SeqCst) {
                return;
            }
            api.prevent_close();
            shutdown_app_state(&state);
            let window = window.clone();
            tauri::async_runtime::spawn(async move {
                let deadline = tokio::time::Instant::now() + WINDOW_SHUTDOWN_GRACE;
                loop {
                    let no_active_runs = window
                        .state::<AppState>()
                        .active_runs
                        .lock()
                        .map(|active| active.is_empty())
                        .unwrap_or(true);
                    if no_active_runs || tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                let _ = window.close();
            });
        })
        .invoke_handler(tauri::generate_handler![
            get_app_snapshot,
            get_voice_profile_snapshot,
            save_voice_enrollment_sample,
            set_target_speaker_filter_enabled,
            delete_voice_enrollment_sample,
            delete_voice_profile,
            read_voice_enrollment_sample,
            frontend_ready,
            export_diagnostics,
            backup_database,
            get_situation_snapshot,
            get_situation_review_snapshot,
            set_situation_monitoring,
            report_owned_signal,
            submit_situation_feedback,
            create_situation_calibration_candidate,
            run_situation_calibration,
            decide_situation_calibration,
            clear_situation_history,
            list_codex_models,
            get_codex_status,
            start_turn,
            cancel_run,
            test_model_provider,
            transcribe_audio,
            preview_audio,
            speak_text,
            stop_tts,
            meeting_preflight,
            start_meeting,
            get_meeting_snapshot,
            watch_meeting,
            unwatch_meeting,
            pause_meeting,
            resume_meeting,
            stop_meeting,
            append_meeting_audio_segment,
            preview_meeting_audio_segment,
            save_meeting_transcript,
            discard_meeting,
            save_settings_documents,
            create_conversation,
            list_messages,
            append_message
        ])
        .run(tauri::generate_context!())
        .expect("error while running SAAA");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;

    #[test]
    fn duplicate_run_registration_preserves_the_original_cancellation_handle() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let state = app_state(connection);
        let original = Arc::new(RunCancellation::default());
        register_active_run(&state, "run_duplicate", original.clone())
            .expect("first run registers");
        let replacement = Arc::new(RunCancellation::default());
        assert!(register_active_run(&state, "run_duplicate", replacement).is_err());
        let active = state.active_runs.lock().expect("active run lock");
        assert!(Arc::ptr_eq(
            active.get("run_duplicate").expect("original run remains"),
            &original
        ));
    }

    #[test]
    fn app_shutdown_cancels_every_active_run_and_resets_situation_to_safe_state() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let state = app_state(connection);
        let first = Arc::new(RunCancellation::default());
        let second = Arc::new(RunCancellation::default());
        register_active_run(&state, "run-close-first", first.clone()).expect("first run registers");
        register_active_run(&state, "run-close-second", second.clone())
            .expect("second run registers");

        state
            .situation
            .set_microphone_state(situation::contracts::MicrophoneState::SaaaCapturing);
        state
            .situation
            .set_audio_state(situation::contracts::AudioState::SaaaSpeaking);
        shutdown_app_state(&state);

        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
        let snapshot = state
            .situation
            .snapshot_locked(&state.connection)
            .expect("situation snapshot");
        assert_eq!(
            snapshot.signals.microphone.state,
            situation::contracts::MicrophoneState::Inactive
        );
        assert_eq!(
            snapshot.signals.audio.state,
            situation::contracts::AudioState::Silent
        );
    }

    #[test]
    fn runtime_and_provider_session_finalization_is_one_shot() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, task_mode, created_at, updated_at)
                 VALUES('conversation-finalize', 'conversation', '1', '1')",
                [],
            )
            .expect("conversation inserts");
        let state = app_state(connection);

        begin_simple_runtime_run(
            &state,
            "run-finalize",
            "conversation-finalize",
            "voice.speak",
            "provider-initial",
        )
        .expect("runtime starts");
        update_runtime_provider(&state, "run-finalize", "provider-selected")
            .expect("active runtime provider updates");
        finish_runtime_run(&state, "run-finalize", "completed", None).expect("runtime finalizes");
        assert!(
            finish_runtime_run(&state, "run-finalize", "failed", Some("late failure")).is_err()
        );
        assert!(update_runtime_provider(&state, "run-finalize", "provider-late").is_err());

        let session_id = begin_provider_session(
            &state,
            "run-finalize",
            "provider-selected",
            "openai-compatible",
        )
        .expect("provider session starts");
        finish_provider_session(&state, &session_id, "completed", None)
            .expect("provider session finalizes");
        assert!(finish_provider_session(
            &state,
            &session_id,
            "failed",
            Some(ProviderFailureKind::Internal),
        )
        .is_err());

        let connection = state.connection.lock().expect("database lock");
        let (status, provider_id): (String, String) = connection
            .query_row(
                "SELECT status, provider_id FROM runtime_runs WHERE id='run-finalize'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("runtime reads");
        assert_eq!(status, "completed");
        assert_eq!(provider_id, "provider-selected");
        let session_status: String = connection
            .query_row(
                "SELECT status FROM provider_sessions WHERE id=?1",
                [&session_id],
                |row| row.get(0),
            )
            .expect("provider session reads");
        assert_eq!(session_status, "completed");
    }

    #[test]
    fn gnosis_session_persists_release_success_and_deferred_cleanup() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, task_mode, created_at, updated_at)
                 VALUES('conversation-gnosis-cleanup', 'conversation', '1', '1')",
                [],
            )
            .expect("conversation inserts");
        let state = app_state(connection);

        for (run_id, cleanup, expected_status, expected_kind) in [
            (
                "run-gnosis-released",
                CleanupOutcome::Released,
                "released",
                None,
            ),
            (
                "run-gnosis-deferred",
                CleanupOutcome::GnosisDeferredToTtl { kind: "network" },
                "deferred-to-ttl",
                Some("network"),
            ),
        ] {
            begin_simple_runtime_run(
                &state,
                run_id,
                "conversation-gnosis-cleanup",
                "conversation.respond",
                "gnosis-qwen",
            )
            .expect("runtime starts");
            let session_id =
                begin_provider_session(&state, run_id, "gnosis-qwen", "openai-compatible")
                    .expect("provider session starts");
            finish_gnosis_provider_session(&state, &session_id, "completed", None, cleanup)
                .expect("gnosis session finalizes");
            let connection = state.connection.lock().expect("database lock");
            let (release_status, release_kind): (String, Option<String>) = connection
                .query_row(
                    "SELECT release_status, release_failure_kind FROM provider_sessions WHERE id=?1",
                    [&session_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .expect("cleanup status reads");
            assert_eq!(release_status, expected_status);
            assert_eq!(release_kind.as_deref(), expected_kind);
        }
    }

    #[test]
    fn gnosis_cleanup_keeps_release_debt_across_connection_replacement() {
        let deferred = CleanupOutcome::GnosisDeferredToTtl { kind: "network" };
        assert_eq!(
            merge_gnosis_cleanup(deferred, CleanupOutcome::Released),
            deferred
        );
        assert_eq!(
            merge_gnosis_cleanup(CleanupOutcome::Released, CleanupOutcome::Released),
            CleanupOutcome::Released
        );
        assert_eq!(
            gnosis_cleanup_from_release_failure(Some(providers::gnosis::ErrorKind::Timeout)),
            CleanupOutcome::GnosisDeferredToTtl { kind: "timeout" }
        );
    }

    #[tokio::test]
    async fn cancellation_notification_remains_observable_for_late_waiters() {
        let cancellation = RunCancellation::default();
        cancellation.cancel();

        tokio::time::timeout(Duration::from_millis(50), cancellation.cancelled())
            .await
            .expect("a cancellation sent before waiting remains observable");
    }

    #[test]
    fn auxiliary_codex_reader_bounds_and_decodes_stdout() {
        let (receiver, reader) = spawn_bounded_codex_reader(std::io::Cursor::new(
            b"{\"id\":2,\"result\":{}}\n".to_vec(),
        ));
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_millis(50))
                .expect("reader returns a message")
                .expect("message decodes")["id"],
            2
        );
        drop(receiver);
        reader.join().expect("reader joins");

        let oversized = vec![b'x'; MAX_CODEX_STDOUT_BYTES as usize + 1];
        let (receiver, reader) = spawn_bounded_codex_reader(std::io::Cursor::new(oversized));
        assert!(receiver
            .recv_timeout(Duration::from_millis(250))
            .expect("reader returns a bounded failure")
            .expect_err("oversized output is rejected")
            .contains("4 MiB"));
        drop(receiver);
        reader.join().expect("oversized reader joins");
    }

    #[test]
    fn runtime_and_voice_events_serialize_camel_case_fields() {
        let runtime = serde_json::to_value(RuntimeEvent::Started {
            run_id: "run_contract".to_string(),
            route: "coding.assist".to_string(),
            provider_id: "codex-sdk".to_string(),
        })
        .expect("runtime event serializes");
        assert_eq!(runtime["type"], "started");
        assert_eq!(runtime["runId"], "run_contract");
        assert_eq!(runtime["providerId"], "codex-sdk");
        assert!(runtime.get("run_id").is_none());
        assert!(runtime.get("provider_id").is_none());

        let selected = serde_json::to_value(RuntimeEvent::ProviderSelected {
            run_id: "run_contract".to_string(),
            provider_id: "larm-primary".to_string(),
            provider_kind: "larm".to_string(),
            route_id: "llm-default".to_string(),
            runtime_id: "runtime-safe".to_string(),
            fallback_used: false,
            selection_reason_code: "primary".to_string(),
        })
        .expect("provider selection serializes");
        assert_eq!(selected["type"], "providerSelected");
        assert_eq!(selected["routeId"], "llm-default");
        assert_eq!(selected["selectionReasonCode"], "primary");
        assert!(selected.get("allocationId").is_none());
        assert!(selected.get("requestId").is_none());

        let voice = serde_json::to_value(VoiceEvent::TranscriptFinal {
            run_id: "voice_contract".to_string(),
            text: "transcript".to_string(),
        })
        .expect("voice event serializes");
        assert_eq!(voice["type"], "transcriptFinal");
        assert_eq!(voice["runId"], "voice_contract");
        assert!(voice.get("run_id").is_none());
    }

    #[test]
    fn foreign_message_flood_cannot_starve_a_request_deadline() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let producer = thread::spawn(move || {
            while sender
                .send(CodexReaderMessage::Message(json!({
                    "id": 999,
                    "result": {}
                })))
                .is_ok()
            {}
        });
        let origin = std::time::Instant::now();
        let mut supervisor = runtime::supervisor::RunSupervisor::new(
            runtime::contracts::RunSupervisionPolicy {
                request_timeout_ms: 20,
                progress_idle_timeout_ms: 60,
                terminal_gap_timeout_ms: 10,
                interrupt_grace_ms: 3,
                hard_timeout_ms: 300,
            },
            0,
        );
        let failure = receive_supervised_codex_result(
            &receiver,
            2,
            &mut supervisor,
            origin,
            &RunCancellation::default(),
        )
        .expect_err("foreign responses must not keep the request alive");
        drop(receiver);
        producer.join().expect("foreign response producer joins");
        assert_eq!(failure, runtime::contracts::RunFailureCode::RequestTimeout);
        assert!(origin.elapsed() < Duration::from_millis(250));
    }

    #[test]
    fn normal_turns_reject_legacy_conversation_ids_without_writing_partial_state() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('legacy-conversation', 'Legacy', 'conversation', '0', '0')",
                [],
            )
            .expect("legacy conversation inserts");
        let state = app_state(connection);
        let input = StartTurnInput {
            run_id: "run-legacy-conversation".to_string(),
            conversation_id: "legacy-conversation".to_string(),
            content: "must not persist".to_string(),
            workspace_path: None,
        };

        let error = prepare_runtime_run(&state, &input).expect_err("legacy turn is rejected");
        let connection = state.connection.lock().expect("database lock");
        let message_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE content = 'must not persist'",
                [],
                |row| row.get(0),
            )
            .expect("message count loads");
        let run_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM runtime_runs WHERE id = 'run-legacy-conversation'",
                [],
                |row| row.get(0),
            )
            .expect("run count loads");

        assert!(error.contains("primary conversation"));
        assert_eq!(message_count, 0);
        assert_eq!(run_count, 0);
    }

    #[test]
    fn active_meeting_rejects_coding_turn_before_writing_partial_state() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('meeting-blocked-coding', 'Coding', 'coding', '0', '0')",
                [],
            )
            .expect("coding conversation inserts");
        let state = app_state(connection);
        state
            .meeting
            .preflight(
                &meeting::PreflightInput {
                    microphone_device_id: "default".to_string(),
                    system_audio_enabled: false,
                    stt_model: voice::gnosis_asr::MODEL_ID.to_string(),
                    translation_enabled: false,
                },
                Ok(()),
            )
            .expect("meeting preflight succeeds");
        state
            .meeting
            .start(
                &meeting::StartInput {
                    session_id: "meeting-agent-policy".to_string(),
                    microphone_device_id: "default".to_string(),
                    microphone_enabled: true,
                    system_audio_enabled: false,
                    stt_model: voice::gnosis_asr::MODEL_ID.to_string(),
                    translation_enabled: false,
                    persistence_mode: "discard".to_string(),
                },
                &state.connection,
            )
            .expect("meeting starts");
        let input = StartTurnInput {
            run_id: "run-meeting-blocked-coding".to_string(),
            conversation_id: "meeting-blocked-coding".to_string(),
            content: "inspect only".to_string(),
            workspace_path: Some("/tmp/fixture".to_string()),
        };

        let error = prepare_runtime_run(&state, &input).expect_err("coding turn is blocked");
        let connection = state.connection.lock().expect("database lock");
        let message_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = 'meeting-blocked-coding'",
                [],
                |row| row.get(0),
            )
            .expect("message count loads");
        let run_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM runtime_runs WHERE id = 'run-meeting-blocked-coding'",
                [],
                |row| row.get(0),
            )
            .expect("run count loads");

        assert_eq!(
            error,
            "MEETING_POLICY_AGENT_BLOCKED: Coding Agent is disabled during a meeting."
        );
        assert_eq!(message_count, 0);
        assert_eq!(run_count, 0);
    }

    #[test]
    fn running_coding_turn_rejects_meeting_start_before_writing_partial_state() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('agent-blocks-meeting', 'Coding', 'coding', '0', '0')",
                [],
            )
            .expect("coding conversation inserts");
        let state = app_state(connection);
        let workspace = tempfile::tempdir().expect("workspace creates");
        let turn = StartTurnInput {
            run_id: "run-agent-blocks-meeting".to_string(),
            conversation_id: "agent-blocks-meeting".to_string(),
            content: "inspect only".to_string(),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
        };
        assert_eq!(
            prepare_runtime_run(&state, &turn).expect("coding run prepares"),
            "coding"
        );
        state
            .meeting
            .preflight(
                &meeting::PreflightInput {
                    microphone_device_id: "default".to_string(),
                    system_audio_enabled: false,
                    stt_model: voice::gnosis_asr::MODEL_ID.to_string(),
                    translation_enabled: false,
                },
                Ok(()),
            )
            .expect("meeting preflight succeeds");

        let error = start_meeting_inner(
            &state,
            &meeting::StartInput {
                session_id: "meeting-blocked-by-agent".to_string(),
                microphone_device_id: "default".to_string(),
                microphone_enabled: true,
                system_audio_enabled: false,
                stt_model: voice::gnosis_asr::MODEL_ID.to_string(),
                translation_enabled: false,
                persistence_mode: "discard".to_string(),
            },
        )
        .expect_err("meeting start is blocked");
        let snapshot = state.meeting.snapshot().expect("meeting snapshot loads");
        let connection = state.connection.lock().expect("database lock");
        let meeting_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM meeting_sessions", [], |row| {
                row.get(0)
            })
            .expect("meeting count loads");

        assert_eq!(
            error,
            "MEETING_POLICY_AGENT_BLOCKED: Stop the Coding Agent and retry."
        );
        assert_eq!(snapshot.state, meeting::MeetingState::Ready);
        assert_eq!(meeting_count, 0);
    }

    #[test]
    fn task_specific_workspace_validation_precedes_runtime_writes() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('workspace-required', 'Coding', 'coding', '0', '0')",
                [],
            )
            .expect("coding conversation inserts");
        let state = app_state(connection);
        let workspace = tempfile::tempdir().expect("workspace creates");
        let normal = StartTurnInput {
            run_id: "run-normal-workspace".to_string(),
            conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
            content: "normal request".to_string(),
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
        };
        let coding = StartTurnInput {
            run_id: "run-coding-no-workspace".to_string(),
            conversation_id: "workspace-required".to_string(),
            content: "coding request".to_string(),
            workspace_path: None,
        };

        assert!(prepare_runtime_run(&state, &normal)
            .expect_err("normal workspace is rejected")
            .contains("cannot include a workspace"));
        assert!(prepare_runtime_run(&state, &coding)
            .expect_err("coding workspace is required")
            .contains("Select a workspace"));
        let connection = state.connection.lock().expect("database lock");
        let message_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages
                 WHERE content IN ('normal request', 'coding request')",
                [],
                |row| row.get(0),
            )
            .expect("message count loads");
        let run_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM runtime_runs
                 WHERE id IN ('run-normal-workspace', 'run-coding-no-workspace')",
                [],
                |row| row.get(0),
            )
            .expect("run count loads");
        assert_eq!(message_count, 0);
        assert_eq!(run_count, 0);
    }

    #[test]
    fn normal_turns_reject_byte_oversized_context_before_writing_partial_state() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let state = app_state(connection);
        let content = "😀".repeat(16_000);
        let input = StartTurnInput {
            run_id: "run-byte-oversized-context".to_string(),
            conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
            content: content.clone(),
            workspace_path: None,
        };

        let error = prepare_runtime_run(&state, &input).expect_err("oversized turn is rejected");
        let connection = state.connection.lock().expect("database lock");
        let message_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE content = ?1",
                params![content],
                |row| row.get(0),
            )
            .expect("message count loads");
        let run_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM runtime_runs WHERE id = 'run-byte-oversized-context'",
                [],
                |row| row.get(0),
            )
            .expect("run count loads");

        assert!(error.contains("too large"));
        assert_eq!(message_count, 0);
        assert_eq!(run_count, 0);
    }

    #[test]
    fn readiness_data_directory_is_isolated_and_permission_bounded() {
        let directory = tempfile::tempdir().expect("temporary directory creates");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .expect("temporary directory permissions are isolated");
        }
        let normal = directory.path().join("normal-app-data");
        let resolved = validate_readiness_data_directory(directory.path(), &normal)
            .expect("isolated directory is accepted");
        assert_eq!(
            resolved,
            directory.path().canonicalize().expect("path resolves")
        );
        assert!(
            validate_readiness_data_directory(directory.path(), directory.path())
                .expect_err("normal app data is rejected")
                .contains("normal application data")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
                .expect("permissions change");
            assert!(validate_readiness_data_directory(directory.path(), &normal)
                .expect_err("broad permissions are rejected")
                .contains("mode 0700"));
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .expect("permissions restore");
        }
    }

    #[tokio::test]
    async fn openai_compatible_stream_fixture_projects_deltas() {
        use std::io::{Read, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let request_body = Arc::new(Mutex::new(String::new()));
        let request_body_for_server = request_body.clone();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("fixture accepts request");
            let mut request = vec![0; 16_384];
            let size = socket.read(&mut request).expect("fixture reads request");
            *request_body_for_server.lock().expect("request lock") =
                String::from_utf8_lossy(&request[..size]).into_owned();
            let response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: text/event-stream\r\n",
                "Connection: close\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"hello \"}}]}\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"world\"}}]}\n\n",
                "data: [DONE]\n\n"
            );
            socket
                .write_all(response.as_bytes())
                .expect("fixture writes response");
        });
        let provider = OpenAiCompatibleProviderSettings {
            endpoint: format!("http://{address}/v1"),
            ..direct_provider("stream-fixture", "local")
        };
        let input = StartTurnInput {
            run_id: "run-stream-fixture".to_string(),
            conversation_id: "conversation-fixture".to_string(),
            content: "hello".to_string(),
            workspace_path: None,
        };
        let history = vec![ConversationMessage {
            id: "message-fixture".to_string(),
            conversation_id: input.conversation_id.clone(),
            role: "user".to_string(),
            content: input.content.clone(),
            created_at: "now".to_string(),
        }];
        let projected = Arc::new(Mutex::new(Vec::<String>::new()));
        let projected_for_channel = projected.clone();
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(value) = body {
                projected_for_channel
                    .lock()
                    .expect("projection lock")
                    .push(value);
            }
            Ok(())
        });
        let content = stream_model_provider_with_api_key(
            &provider,
            &history,
            5_000,
            Some("ephemeral-connection-token"),
            false,
            ModelStreamContext {
                reasoning_effort: "low",
                input: &input,
                on_event: &channel,
                cancellation: Arc::new(RunCancellation::default()),
                output_persistence: None,
            },
        )
        .await;
        let ProviderAttemptOutcome::Completed { content, .. } = content else {
            panic!("provider stream should complete");
        };
        server.join().expect("fixture server joins");
        assert_eq!(content, "hello world");
        assert!(request_body
            .lock()
            .expect("request lock")
            .contains("POST /v1/chat/completions"));
        assert!(request_body
            .lock()
            .expect("request lock")
            .contains("\"reasoning_effort\":\"low\""));
        assert!(request_body
            .lock()
            .expect("request lock")
            .to_ascii_lowercase()
            .contains("authorization: bearer ephemeral-connection-token"));
        assert_eq!(
            projected
                .lock()
                .expect("projection lock")
                .iter()
                .filter(|event| event.contains("\"type\":\"delta\""))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn gnosis_stream_policy_rejects_a_non_sse_completion() {
        use std::io::{Read, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("fixture accepts request");
            let mut request = [0_u8; 8_192];
            let _ = socket.read(&mut request).expect("fixture reads request");
            let body = r#"{"choices":[{"message":{"content":"not streamed"}}]}"#;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("fixture writes response");
        });
        let provider = OpenAiCompatibleProviderSettings {
            endpoint: format!("http://{address}/v1"),
            ..direct_provider("gnosis-sse-policy", "local")
        };
        let input = StartTurnInput {
            run_id: "run-gnosis-sse-policy".to_string(),
            conversation_id: "conversation-gnosis-sse-policy".to_string(),
            content: "test".to_string(),
            workspace_path: None,
        };
        let history = vec![ConversationMessage {
            id: "message-gnosis-sse-policy".to_string(),
            conversation_id: input.conversation_id.clone(),
            role: "user".to_string(),
            content: input.content.clone(),
            created_at: "now".to_string(),
        }];
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
        let outcome = stream_model_provider_with_api_key(
            &provider,
            &history,
            5_000,
            Some("ephemeral-connection-token"),
            true,
            ModelStreamContext {
                reasoning_effort: "low",
                input: &input,
                on_event: &channel,
                cancellation: Arc::new(RunCancellation::default()),
                output_persistence: None,
            },
        )
        .await;
        server.join().expect("fixture server joins");
        assert!(matches!(
            outcome,
            ProviderAttemptOutcome::Failed {
                kind: ProviderFailureKind::Protocol,
                output_started: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn openai_provider_executes_the_single_recall_tool_before_final_output() {
        use std::io::{Read, Write as _};
        use std::net::{TcpListener, TcpStream};

        fn read_request(socket: &mut TcpStream) -> String {
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 8_192];
            let mut expected = None;
            loop {
                let size = socket.read(&mut buffer).expect("fixture reads request");
                assert!(size > 0, "request closed before its body completed");
                bytes.extend_from_slice(&buffer[..size]);
                if expected.is_none() {
                    if let Some(boundary) = bytes.windows(4).position(|value| value == b"\r\n\r\n")
                    {
                        let headers = String::from_utf8_lossy(&bytes[..boundary]);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                line.split_once(':').and_then(|(name, value)| {
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                })
                            })
                            .expect("content length exists");
                        expected = Some(boundary + 4 + content_length);
                    }
                }
                if expected.is_some_and(|length| bytes.len() >= length) {
                    return String::from_utf8(bytes).expect("request is UTF-8");
                }
            }
        }

        fn request_json(request: &str) -> Value {
            let (_, body) = request.split_once("\r\n\r\n").expect("request has body");
            serde_json::from_str(body).expect("request body is JSON")
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let captures = Arc::new(Mutex::new(Vec::<String>::new()));
        let captures_for_server = captures.clone();
        let first_delta = json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call_recall_1",
                "type": "function",
                "function": {"name": "recall_conversation", "arguments": "{\"query\":\""}
            }]}}]
        });
        let second_delta = json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "function": {"arguments": "SQLite\"}"}
            }]}}]
        });
        let server = thread::spawn(move || {
            let responses = [
                format!(
                    concat!(
                        "HTTP/1.1 200 OK\r\n",
                        "Content-Type: text/event-stream\r\n",
                        "Connection: close\r\n\r\n",
                        "data: {}\n\n",
                        "data: {}\n\n",
                        "data: [DONE]\n\n"
                    ),
                    first_delta, second_delta
                ),
                concat!(
                    "HTTP/1.1 200 OK\r\n",
                    "Content-Type: text/event-stream\r\n",
                    "Connection: close\r\n\r\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"履歴を確認しました\"}}]}\n\n",
                    "data: [DONE]\n\n"
                )
                .to_string(),
            ];
            for response in responses {
                let (mut socket, _) = listener.accept().expect("fixture accepts request");
                captures_for_server
                    .lock()
                    .expect("capture lock")
                    .push(read_request(&mut socket));
                socket
                    .write_all(response.as_bytes())
                    .expect("fixture writes response");
            }
        });

        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute_batch(
                "INSERT INTO conversations(id,task_mode,created_at,updated_at)
                   VALUES('conversation-old','conversation','1','1');
                 INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                   VALUES('message-old-user','conversation-old','user','SQLite の検索方式を相談した','1000');
                 INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                   VALUES('message-old-assistant','conversation-old','assistant','FTS と時間条件を組み合わせます','1001');",
            )
            .expect("history inserts");
        let state = app_state(connection);
        let input = StartTurnInput {
            run_id: "run-recall-tool".to_string(),
            conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
            content: "前の話を思い出して".to_string(),
            workspace_path: None,
        };
        prepare_runtime_run(&state, &input).expect("runtime prepares");
        let session_id =
            begin_provider_session(&state, &input.run_id, "recall-fixture", "openai-compatible")
                .expect("provider session starts");
        let history = list_messages_from_connection(
            &state.connection.lock().expect("database lock"),
            &input.conversation_id,
        )
        .expect("history loads");
        let provider = OpenAiCompatibleProviderSettings {
            endpoint: format!("http://{address}/v1"),
            ..direct_provider("recall-fixture", "local")
        };
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
        let outcome = stream_model_provider(
            &provider,
            &history,
            5_000,
            ModelStreamContext {
                reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
                input: &input,
                on_event: &channel,
                cancellation: Arc::new(RunCancellation::default()),
                output_persistence: Some(ProviderOutputPersistence {
                    state: &state,
                    session_id: &session_id,
                }),
            },
        )
        .await;
        server.join().expect("fixture server joins");

        let ProviderAttemptOutcome::Completed { content, .. } = outcome else {
            panic!("tool-assisted provider stream should complete");
        };
        assert_eq!(content, "履歴を確認しました");
        let captures = captures.lock().expect("capture lock");
        assert_eq!(captures.len(), 2);
        let first = request_json(&captures[0]);
        assert_eq!(first["tools"].as_array().expect("tools array").len(), 1);
        assert_eq!(
            first
                .pointer("/tools/0/function/name")
                .and_then(Value::as_str),
            Some("recall_conversation")
        );
        let second = request_json(&captures[1]);
        let messages = second["messages"].as_array().expect("messages array");
        assert!(messages.iter().any(|message| {
            message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("SQLite の検索方式"))
        }));
        assert!(!messages.iter().any(|message| {
            message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains("前の話を思い出して"))
        }));
        let connection = state.connection.lock().expect("database lock");
        let receipts: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_recall_receipts WHERE runtime_run_id=?1",
                [&input.run_id],
                |row| row.get(0),
            )
            .expect("receipt count reads");
        assert_eq!(receipts, 1);
    }

    #[tokio::test]
    async fn typed_memory_tools_are_routed_only_from_a_valid_typed_manifest() {
        let directory = tempfile::tempdir().expect("temporary run directory creates");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .expect("run directory permissions set");
        }
        let token_path = directory.path().join("mcp-memory-bearer.token");
        fs::write(
            &token_path,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
        )
        .expect("test token writes");
        let manifest_path = directory.path().join("mcp-endpoint.json");
        fs::write(
            &manifest_path,
            serde_json::to_vec(&json!({
                "server": "context-still",
                "url": "http://127.0.0.1:39173/mcp",
                "transport": "streamable-http",
                "protocolVersion": "2025-03-26",
                "auth": "bearer-token-file",
                "authTokenPath": token_path,
                "toolProfile": "typed-memory",
                "contractVersion": "memory-recall-v1",
                "startedAt": "unix-ms:1"
            }))
            .expect("manifest encodes"),
        )
        .expect("manifest writes");
        #[cfg(unix)]
        for path in [&token_path, &manifest_path] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .expect("file permissions set");
        }

        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut state = app_state(connection);
        state.context_still_recall =
            memory::context_still_recall::ContextStillRecallClient::with_run_dir(
                directory.path().to_path_buf(),
                true,
            );
        let input = StartTurnInput {
            run_id: "run-typed-routing".to_string(),
            conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
            content: "remember a rule".to_string(),
            workspace_path: None,
        };
        let persistence = Some(ProviderOutputPersistence {
            state: &state,
            session_id: "unused-session",
        });
        let names = available_agent_tools(persistence, &input, 0)
            .into_iter()
            .map(|definition| {
                definition
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .expect("tool name exists")
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "recall_conversation",
                "recall_experience",
                "recall_rule",
                "recall_skill"
            ]
        );

        let error = execute_agent_tool(
            persistence,
            &input,
            &runtime::agent_tools::AgentToolCall {
                id: "call-typed-invalid".to_string(),
                name: "recall_rule".to_string(),
                arguments: r#"{"query":"release","projectRef":"forbidden"}"#.to_string(),
            },
            Duration::from_secs(1),
        )
        .await;
        assert!(error.contains("invalid-memory-input"));
        assert!(!error.contains("forbidden"));
    }

    #[tokio::test]
    async fn typed_memory_execution_cannot_exceed_the_provider_deadline() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let (release_sender, release_receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("fixture accepts request");
            let mut request = [0_u8; 4 * 1_024];
            let _ = socket.read(&mut request).expect("fixture reads request");
            release_receiver.recv().expect("fixture release arrives");
        });

        let directory = tempfile::tempdir().expect("run directory creates");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
                .expect("run directory permissions set");
        }
        let token_path = directory.path().join("mcp-memory-bearer.token");
        let manifest_path = directory.path().join("mcp-endpoint.json");
        fs::write(
            &token_path,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
        )
        .expect("token writes");
        fs::write(
            &manifest_path,
            serde_json::to_vec(&json!({
                "server": "context-still",
                "url": format!("http://{address}/mcp"),
                "transport": "streamable-http",
                "protocolVersion": "2025-03-26",
                "auth": "bearer-token-file",
                "authTokenPath": token_path,
                "toolProfile": "typed-memory",
                "contractVersion": "memory-recall-v1",
                "startedAt": "unix-ms:1"
            }))
            .expect("manifest encodes"),
        )
        .expect("manifest writes");
        #[cfg(unix)]
        for path in [&token_path, &manifest_path] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .expect("fixture permissions set");
        }

        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut state = app_state(connection);
        state.context_still_recall =
            memory::context_still_recall::ContextStillRecallClient::with_run_dir(
                directory.path().to_path_buf(),
                true,
            );
        let input = StartTurnInput {
            run_id: "run-typed-memory-timeout".to_string(),
            conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
            content: "remember".to_string(),
            workspace_path: None,
        };
        let persistence = Some(ProviderOutputPersistence {
            state: &state,
            session_id: "session-typed-memory-timeout",
        });
        let call = runtime::agent_tools::AgentToolCall {
            id: "call-typed-timeout".to_string(),
            name: "recall_rule".to_string(),
            arguments: r#"{"query":"release"}"#.to_string(),
        };
        let execution = execute_agent_tool(persistence, &input, &call, Duration::from_millis(20));
        let error = tokio::time::timeout(Duration::from_millis(250), execution)
            .await
            .expect("typed recall respects the provider deadline");
        assert!(error.contains("typed-memory-unavailable"));

        release_sender.send(()).expect("fixture releases");
        server.join().expect("fixture joins");
    }

    #[test]
    fn malformed_recall_calls_consume_the_persistent_turn_limit() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let state = app_state(connection);
        let input = StartTurnInput {
            run_id: "run-malformed-recall".to_string(),
            conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
            content: "remember".to_string(),
            workspace_path: None,
        };
        prepare_runtime_run(&state, &input).expect("runtime prepares");
        let persistence = Some(ProviderOutputPersistence {
            state: &state,
            session_id: "unused-session",
        });
        for index in 0..3 {
            let content = execute_recall_tool(
                persistence,
                &input,
                &runtime::agent_tools::AgentToolCall {
                    id: format!("call_malformed_{index}"),
                    name: "recall_conversation".to_string(),
                    arguments: "{".to_string(),
                },
            );
            assert!(content.contains("invalid-input"));
        }
        let limited = execute_recall_tool(
            persistence,
            &input,
            &runtime::agent_tools::AgentToolCall {
                id: "call_malformed_4".to_string(),
                name: "recall_conversation".to_string(),
                arguments: "{".to_string(),
            },
        );
        assert!(limited.contains("call-limit-exceeded"));
        let attempts: i64 = state
            .connection
            .lock()
            .expect("database lock")
            .query_row(
                "SELECT COUNT(*) FROM conversation_recall_attempts
                 WHERE runtime_run_id=?1",
                [&input.run_id],
                |row| row.get(0),
            )
            .expect("attempt count reads");
        assert_eq!(attempts, 3);
    }

    #[tokio::test]
    async fn recall_tool_rounds_share_one_provider_timeout_budget() {
        use std::io::{Read, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let tool_response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: text/event-stream\r\n",
                "Connection: close\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{",
                "\"index\":0,\"id\":\"call_timeout\",\"type\":\"function\",",
                "\"function\":{\"name\":\"recall_conversation\",",
                "\"arguments\":\"{\\\"query\\\":\\\"missing\\\"}\"}}]}}]}\n\n",
                "data: [DONE]\n\n"
            );
            let final_response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: text/event-stream\r\n",
                "Connection: close\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"too late\"}}]}\n\n",
                "data: [DONE]\n\n"
            );
            for response in [tool_response, final_response] {
                let (mut socket, _) = listener.accept().expect("fixture accepts request");
                let mut request = [0_u8; 32 * 1_024];
                let _ = socket.read(&mut request).expect("fixture reads request");
                thread::sleep(Duration::from_millis(350));
                let _ = socket.write_all(response.as_bytes());
            }
        });

        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let state = app_state(connection);
        let input = StartTurnInput {
            run_id: "run-recall-timeout".to_string(),
            conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
            content: "remember".to_string(),
            workspace_path: None,
        };
        prepare_runtime_run(&state, &input).expect("runtime prepares");
        let session_id = begin_provider_session(
            &state,
            &input.run_id,
            "timeout-fixture",
            "openai-compatible",
        )
        .expect("provider session starts");
        let history = list_messages_from_connection(
            &state.connection.lock().expect("database lock"),
            &input.conversation_id,
        )
        .expect("history loads");
        let provider = OpenAiCompatibleProviderSettings {
            endpoint: format!("http://{address}/v1"),
            ..direct_provider("timeout-fixture", "local")
        };
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
        let outcome = stream_model_provider(
            &provider,
            &history,
            600,
            ModelStreamContext {
                reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
                input: &input,
                on_event: &channel,
                cancellation: Arc::new(RunCancellation::default()),
                output_persistence: Some(ProviderOutputPersistence {
                    state: &state,
                    session_id: &session_id,
                }),
            },
        )
        .await;
        server.join().expect("fixture server joins");
        assert!(matches!(
            outcome,
            ProviderAttemptOutcome::Failed {
                kind: ProviderFailureKind::Timeout,
                output_started: false,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn provider_stream_stops_when_the_tauri_consumer_disconnects() {
        use std::io::{Read, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("fixture accepts request");
            let mut request = [0; 4_096];
            let _ = socket.read(&mut request).expect("fixture reads request");
            socket
                .write_all(
                    concat!(
                        "HTTP/1.1 200 OK\r\n",
                        "Content-Type: text/event-stream\r\n",
                        "Connection: close\r\n\r\n",
                        "data: {\"choices\":[{\"delta\":{\"content\":\"visible\"}}]}\n\n",
                        "data: {\"choices\":[{\"delta\":{\"content\":\"ignored\"}}]}\n\n",
                        "data: [DONE]\n\n"
                    )
                    .as_bytes(),
                )
                .expect("fixture writes response");
        });
        let provider = OpenAiCompatibleProviderSettings {
            endpoint: format!("http://{address}/v1"),
            ..direct_provider("consumer-disconnect", "local")
        };
        let input = StartTurnInput {
            run_id: "run-consumer-disconnect".to_string(),
            conversation_id: "conversation-consumer-disconnect".to_string(),
            content: "hello".to_string(),
            workspace_path: None,
        };
        let channel: tauri::ipc::Channel<RuntimeEvent> =
            tauri::ipc::Channel::new(|_| Err(tauri::Error::Io(std::io::Error::other("closed"))));
        let outcome = stream_model_provider(
            &provider,
            &[],
            2_000,
            ModelStreamContext {
                reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
                input: &input,
                on_event: &channel,
                cancellation: Arc::new(RunCancellation::default()),
                output_persistence: None,
            },
        )
        .await;
        server.join().expect("fixture server joins");
        assert!(matches!(
            outcome,
            ProviderAttemptOutcome::Failed {
                kind: ProviderFailureKind::ClientDisconnected,
                output_started: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn model_provider_redirects_are_not_followed() {
        use std::io::{ErrorKind, Read, Write as _};
        use std::net::TcpListener;

        let redirect_target = TcpListener::bind("127.0.0.1:0").expect("target binds");
        let target_address = redirect_target.local_addr().expect("target address");
        redirect_target
            .set_nonblocking(true)
            .expect("target becomes nonblocking");
        let target_hit = Arc::new(AtomicBool::new(false));
        let target_hit_for_server = target_hit.clone();
        let target_server = thread::spawn(move || {
            let started = std::time::Instant::now();
            while started.elapsed() < Duration::from_millis(500) {
                match redirect_target.accept() {
                    Ok(_) => {
                        target_hit_for_server.store(true, Ordering::SeqCst);
                        return;
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });

        let source = TcpListener::bind("127.0.0.1:0").expect("source binds");
        let source_address = source.local_addr().expect("source address");
        let source_server = thread::spawn(move || {
            let (mut socket, _) = source.accept().expect("source accepts request");
            let mut request = [0; 4_096];
            let _ = socket.read(&mut request).expect("source reads request");
            let response = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target_address}/v1/chat/completions\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            socket
                .write_all(response.as_bytes())
                .expect("source writes redirect");
        });
        let provider = OpenAiCompatibleProviderSettings {
            endpoint: format!("http://{source_address}/v1"),
            ..direct_provider("redirect-fixture", "local")
        };
        let input = StartTurnInput {
            run_id: "run-redirect-fixture".to_string(),
            conversation_id: "conversation-fixture".to_string(),
            content: "hello".to_string(),
            workspace_path: None,
        };
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
        let outcome = stream_model_provider(
            &provider,
            &[],
            2_000,
            ModelStreamContext {
                reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
                input: &input,
                on_event: &channel,
                cancellation: Arc::new(RunCancellation::default()),
                output_persistence: None,
            },
        )
        .await;
        source_server.join().expect("source server joins");
        target_server.join().expect("target server joins");
        assert!(matches!(
            outcome,
            ProviderAttemptOutcome::Failed {
                kind: ProviderFailureKind::Protocol,
                output_started: false,
                ..
            }
        ));
        assert!(!target_hit.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn conversation_route_falls_back_and_persists_completed_message() {
        use std::io::{Read, Write as _};
        use std::net::TcpListener;

        fn fixture_server(
            response: &'static str,
        ) -> (std::net::SocketAddr, thread::JoinHandle<()>) {
            let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
            let address = listener.local_addr().expect("fixture address");
            let handle = thread::spawn(move || {
                let (mut socket, _) = listener.accept().expect("fixture accepts request");
                let mut request = [0; 16_384];
                let _ = socket.read(&mut request).expect("fixture reads request");
                socket
                    .write_all(response.as_bytes())
                    .expect("fixture writes response");
            });
            (address, handle)
        }

        let (primary_address, primary_server) = fixture_server(concat!(
            "HTTP/1.1 503 Service Unavailable\r\n",
            "Content-Type: text/plain\r\n",
            "Content-Length: 15\r\n",
            "Connection: close\r\n\r\n",
            "primary is down"
        ));
        let (fallback_address, fallback_server) = fixture_server(concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: text/event-stream\r\n",
            "Connection: close\r\n\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"fallback ok\"}}]}\n\n",
            "data: [DONE]\n\n"
        ));
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let state = app_state(connection);
        let conversation = Conversation {
            id: PRIMARY_CONVERSATION_ID.to_string(),
            title: Some(PRIMARY_CONVERSATION_TITLE.to_string()),
            task_mode: "conversation".to_string(),
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        };
        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings")
            .value_json = json!({ "providers": [{
            "kind": "openai-compatible", "id": "primary", "enabled": true, "label": "Primary", "location": "local",
            "endpoint": format!("http://{primary_address}/v1"), "model": "primary-model", "credentialStatus": "not-configured"
        }, {
            "kind": "openai-compatible", "id": "fallback", "enabled": true, "label": "Fallback", "location": "local",
            "endpoint": format!("http://{fallback_address}/v1"), "model": "fallback-model", "credentialStatus": "not-configured"
        }]});
        let route = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("route settings");
        route.value_json["conversationRespond"]["primaryProviderId"] = json!("primary");
        route.value_json["conversationRespond"]["fallbackProviderIds"] = json!(["fallback"]);
        save_settings_documents_to_connection(
            &mut state.connection.lock().expect("database lock"),
            &documents,
        )
        .expect("settings save");
        let input = StartTurnInput {
            run_id: "run-fallback".to_string(),
            conversation_id: conversation.id,
            content: "test fallback".to_string(),
            workspace_path: None,
        };
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let events_for_channel = events.clone();
        let completed_states = Arc::new(Mutex::new(Vec::<String>::new()));
        let completed_states_for_channel = completed_states.clone();
        let delta_states = Arc::new(Mutex::new(Vec::<i64>::new()));
        let delta_states_for_channel = delta_states.clone();
        let database_for_channel = state.connection.clone();
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(value) = body {
                if value.contains("\"type\":\"messageCompleted\"") {
                    let status = database_for_channel
                        .lock()
                        .expect("database lock")
                        .query_row(
                            "SELECT status FROM runtime_runs WHERE id='run-fallback'",
                            [],
                            |row| row.get(0),
                        )
                        .expect("committed run is readable from callback");
                    completed_states_for_channel
                        .lock()
                        .expect("completed state lock")
                        .push(status);
                } else if value.contains("\"type\":\"delta\"") {
                    let output_started = database_for_channel
                        .lock()
                        .expect("database lock")
                        .query_row(
                            "SELECT output_started FROM provider_sessions
                             WHERE runtime_run_id='run-fallback' AND provider_id='fallback'",
                            [],
                            |row| row.get(0),
                        )
                        .expect("committed output state is readable from callback");
                    delta_states_for_channel
                        .lock()
                        .expect("delta state lock")
                        .push(output_started);
                }
                events_for_channel.lock().expect("event lock").push(value);
            }
            Ok(())
        });
        execute_turn(
            &state,
            &input,
            &channel,
            Arc::new(RunCancellation::default()),
            None,
        )
        .await
        .expect("fallback completes");
        primary_server.join().expect("primary server joins");
        fallback_server.join().expect("fallback server joins");
        let messages = list_messages_from_connection(
            &state.connection.lock().expect("database lock"),
            &input.conversation_id,
        )
        .expect("messages load");
        assert_eq!(
            messages.last().expect("assistant message").content,
            "fallback ok"
        );
        let projected = events.lock().expect("event lock").join("\n");
        assert!(projected.contains("providerFailed"));
        assert!(projected.contains("messageCompleted"));
        assert_eq!(projected.matches("\"kind\":\"context-window\"").count(), 1);
        assert!(projected.contains("Context green:"));
        assert_eq!(
            *completed_states.lock().expect("completed state lock"),
            vec!["completed"]
        );
        assert_eq!(*delta_states.lock().expect("delta state lock"), vec![1]);
    }

    #[tokio::test]
    async fn partial_provider_stream_never_reaches_the_fallback_provider() {
        use std::io::{ErrorKind, Read, Write as _};
        use std::net::TcpListener;

        let primary = TcpListener::bind("127.0.0.1:0").expect("primary binds");
        let primary_address = primary.local_addr().expect("primary address");
        let primary_server = thread::spawn(move || {
            let (mut socket, _) = primary.accept().expect("primary accepts request");
            let mut request = [0; 8_192];
            let _ = socket.read(&mut request).expect("primary reads request");
            socket
                .write_all(
                    concat!(
                        "HTTP/1.1 200 OK\r\n",
                        "Content-Type: text/event-stream\r\n",
                        "Connection: close\r\n\r\n",
                        "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"
                    )
                    .as_bytes(),
                )
                .expect("primary writes partial response");
        });

        let fallback = TcpListener::bind("127.0.0.1:0").expect("fallback binds");
        let fallback_address = fallback.local_addr().expect("fallback address");
        fallback
            .set_nonblocking(true)
            .expect("fallback becomes nonblocking");
        let fallback_hit = Arc::new(AtomicBool::new(false));
        let fallback_hit_for_server = fallback_hit.clone();
        let fallback_server = thread::spawn(move || {
            let started = std::time::Instant::now();
            while started.elapsed() < Duration::from_millis(500) {
                match fallback.accept() {
                    Ok(_) => {
                        fallback_hit_for_server.store(true, Ordering::SeqCst);
                        return;
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });

        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let state = app_state(connection);
        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings")
            .value_json = json!({ "providers": [{
            "kind": "openai-compatible", "id": "partial-primary", "enabled": true, "label": "Partial primary", "location": "local",
            "endpoint": format!("http://{primary_address}/v1"), "model": "primary-model", "credentialStatus": "not-configured"
        }, {
            "kind": "openai-compatible", "id": "forbidden-fallback", "enabled": true, "label": "Forbidden fallback", "location": "local",
            "endpoint": format!("http://{fallback_address}/v1"), "model": "fallback-model", "credentialStatus": "not-configured"
        }]});
        let route = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        route.value_json["conversationRespond"]["primaryProviderId"] = json!("partial-primary");
        route.value_json["conversationRespond"]["fallbackProviderIds"] =
            json!(["forbidden-fallback"]);
        save_settings_documents_to_connection(
            &mut state.connection.lock().expect("database lock"),
            &documents,
        )
        .expect("settings save");
        let input = StartTurnInput {
            run_id: "run-partial".to_string(),
            conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
            content: "partial test".to_string(),
            workspace_path: None,
        };
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
        execute_turn(
            &state,
            &input,
            &channel,
            Arc::new(RunCancellation::default()),
            None,
        )
        .await
        .expect_err("partial stream fails the turn");
        primary_server.join().expect("primary server joins");
        fallback_server.join().expect("fallback server joins");

        assert!(!fallback_hit.load(Ordering::SeqCst));
        let connection = state.connection.lock().expect("database lock");
        let assistant_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages
                 WHERE conversation_id=?1 AND role='assistant'",
                params![PRIMARY_CONVERSATION_ID],
                |row| row.get(0),
            )
            .expect("assistant count reads");
        let (session_count, failure_reason): (i64, String) = connection
            .query_row(
                "SELECT COUNT(*), MAX(failure_reason) FROM provider_sessions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("provider session reads");
        assert_eq!(assistant_count, 0);
        assert_eq!(session_count, 1);
        assert_eq!(failure_reason, "network");
    }

    #[test]
    fn codex_thread_mapping_survives_database_reopen() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("threads.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('coding-conversation', NULL, 'coding', 'now', 'now')",
                [],
            )
            .expect("conversation inserts");
        let state = app_state(connection);
        persist_codex_thread(
            &state,
            "coding-conversation",
            "thread-persisted",
            "gpt-test",
            directory.path(),
        )
        .expect("thread persists");
        drop(state);

        let reopened = Connection::open(path).expect("database reopens");
        initialize_database(&reopened).expect("database reinitializes");
        let thread_id: String = reopened
            .query_row(
                "SELECT thread_id FROM codex_threads WHERE conversation_id = 'coding-conversation'",
                [],
                |row| row.get(0),
            )
            .expect("thread reloads");
        assert_eq!(thread_id, "thread-persisted");
    }

    #[test]
    fn audio_resampling_and_cancellation_are_bounded_and_idempotent() {
        let input = (0..48_000)
            .map(|index| (index as f32 / 48_000.0) * 2.0 - 1.0)
            .collect::<Vec<_>>();
        let output = voice::gnosis_asr::resample_pcm(&input, 48_000, 16_000);
        assert_eq!(output.len(), 16_000);
        assert!(output.iter().all(|sample| (-1.0..=1.0).contains(sample)));
        let cancellation = RunCancellation::default();
        cancellation.cancel();
        cancellation.cancel();
        assert!(cancellation.is_cancelled());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_system_tts_runtime_is_available() {
        assert!(Command::new("say")
            .args(["-v", "?"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("system say command starts")
            .success());
    }

    #[test]
    fn codex_model_page_uses_backward_compatible_modalities() {
        let mut page: CodexModelPage = serde_json::from_value(json!({
            "data": [{
                "id": "gpt-test",
                "model": "gpt-test",
                "displayName": "GPT Test",
                "description": "Test model",
                "hidden": false,
                "defaultReasoningEffort": "medium",
                "supportedReasoningEfforts": [],
                "supportsPersonality": true,
                "isDefault": true
            }],
            "nextCursor": null
        }))
        .expect("model page decodes");

        assert_eq!(page.data.len(), 1);
        assert_eq!(page.data[0].input_modalities, ["text", "image"]);
        assert!(page.data[0].is_default);
        validate_codex_model_option(&page.data[0]).expect("bounded model is valid");
        page.data[0].description = "x".repeat(2_001);
        assert!(validate_codex_model_option(&page.data[0]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn codex_app_server_contract_covers_start_stream_resume_and_cancel() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary directory");
        let executable = directory.path().join("codex-fixture.py");
        let log_path = directory.path().join("requests.jsonl");
        let quoted_log_path =
            serde_json::to_string(&log_path.to_string_lossy()).expect("fixture log path encodes");
        let fixture = format!(
            r#"#!/usr/bin/env python3
import json, sys
log_path = {quoted_log_path}
scenario = "normal"
with open(log_path, "a", encoding="utf-8") as log:
    log.write(json.dumps({{"argv": sys.argv[1:]}}) + "\n")
for line in sys.stdin:
    message = json.loads(line)
    with open(log_path, "a", encoding="utf-8") as log:
        log.write(json.dumps(message) + "\n")
    request_id = message.get("id")
    method = message.get("method")
    if request_id == 1:
        print(json.dumps({{"id": 1, "result": {{}}}}), flush=True)
    elif request_id == 2:
        scenario = message.get("params", {{}}).get("model", "normal")
        if scenario != "thread-hang":
            thread_id = "x" * 161 if scenario == "invalid-thread-id" else "fixture-thread"
            print(json.dumps({{"id": 2, "result": {{"thread": {{"id": thread_id}}}}}}), flush=True)
    elif request_id == 3:
        text = message.get("params", {{}}).get("input", [{{}}])[0].get("text", "")
        if scenario != "turn-hang":
            turn_id = "x" * 161 if scenario == "invalid-turn-id" else "fixture-turn"
            print(json.dumps({{"id": 3, "result": {{"turn": {{"id": turn_id}}}}}}), flush=True)
        if scenario == "malformed":
            print("{{not-json", flush=True)
        elif scenario == "provider-error":
            print(json.dumps({{"method": "error", "params": {{"message": "SAAA_PRIVATE_PROVIDER_DETAIL"}}}}), flush=True)
        elif scenario == "terminal-failed":
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "failed", "error": {{"message": "SAAA_PRIVATE_TERMINAL_DETAIL"}}}}}}}}), flush=True)
        elif scenario == "approval":
            print(json.dumps({{"id": 99, "method": "item/requestApproval", "params": {{}}}}), flush=True)
        elif scenario in ["fileChange", "mcpToolCall", "dynamicToolCall", "webSearch"]:
            print(json.dumps({{"method": "item/started", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "forbidden_1", "type": scenario}}}}}}), flush=True)
        elif scenario == "terminal-hang":
            print(json.dumps({{"method": "item/started", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage"}}}}}}), flush=True)
            print(json.dumps({{"method": "item/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage", "text": "SAAA_TERMINAL_WAIT"}}}}}}), flush=True)
        elif scenario == "foreign":
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"threadId": "other", "turnId": "other", "delta": "foreign"}}}}), flush=True)
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"delta": "unscoped"}}}}), flush=True)
        elif scenario == "duplicate":
            started = {{"method": "item/started", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage"}}}}}}
            completed = {{"method": "item/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage", "text": "SAAA_DUPLICATE_OK"}}}}}}
            print(json.dumps(started), flush=True)
            print(json.dumps(started), flush=True)
            print(json.dumps(completed), flush=True)
            print(json.dumps(completed), flush=True)
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "completed"}}}}}}), flush=True)
        elif scenario == "response-too-large":
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "delta": "x" * 64001}}}}), flush=True)
        elif scenario == "stream-too-large":
            print(json.dumps({{"method": "unknown", "params": {{"blob": "x" * (4 * 1024 * 1024)}}}}), flush=True)
        elif scenario == "child-exit":
            sys.exit(3)
        elif scenario not in ["progress-hang", "hard-hang", "cancel-no-response"] and "CANCEL" not in text:
            reply = "SAAA_RESUMED" if method == "turn/start" and "RESUME" in text else "SAAA_OK"
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "delta": reply}}}}), flush=True)
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "completed"}}}}}}), flush=True)
    elif method == "turn/interrupt":
        if scenario != "cancel-no-response":
            print(json.dumps({{"id": request_id, "result": {{}}}}), flush=True)
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "interrupted"}}}}}}), flush=True)
"#,
        );
        fs::write(&executable, fixture).expect("fixture writes");
        let mut permissions = fs::metadata(&executable)
            .expect("fixture metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("fixture becomes executable");

        let previous = env::var_os("SAAA_CODEX_PATH");
        env::set_var("SAAA_CODEX_PATH", &executable);
        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let received_for_channel = received.clone();
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(value) = body {
                received_for_channel.lock().expect("event lock").push(value);
            }
            Ok(())
        });
        let cancellation = RunCancellation::default();
        let first = run_codex_turn_process(
            "run-start",
            "START",
            directory.path(),
            "gpt-fixture",
            None,
            10_000,
            &channel,
            &cancellation,
        )
        .expect("start turn succeeds");
        assert_eq!(first.thread_id, "fixture-thread");
        assert_eq!(first.content, "SAAA_OK");
        let resumed = run_codex_turn_process(
            "run-resume",
            "RESUME",
            directory.path(),
            "gpt-fixture",
            Some(&first.thread_id),
            10_000,
            &channel,
            &cancellation,
        )
        .expect("resume turn succeeds");
        assert_eq!(resumed.content, "SAAA_RESUMED");
        assert_eq!(
            run_codex_turn_process(
                "run-invalid-resume",
                "RESUME",
                directory.path(),
                "gpt-fixture",
                Some(&"x".repeat(161)),
                10_000,
                &channel,
                &cancellation,
            )
            .expect_err("invalid persisted thread id fails closed")
            .code,
            runtime::contracts::RunFailureCode::ProtocolError
        );
        let cancelled = Arc::new(RunCancellation::default());
        let cancel_trigger = cancelled.clone();
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancel_trigger.cancel();
        });
        let cancellation_result = run_codex_turn_process(
            "run-cancel",
            "CANCEL",
            directory.path(),
            "gpt-fixture",
            Some(&first.thread_id),
            10_000,
            &channel,
            &cancelled,
        );
        cancel_thread.join().expect("cancel trigger joins");

        let policy = runtime::contracts::RunSupervisionPolicy {
            request_timeout_ms: 200,
            progress_idle_timeout_ms: 40,
            terminal_gap_timeout_ms: 30,
            interrupt_grace_ms: 20,
            hard_timeout_ms: 200,
        };
        let interrupt_count = || {
            fs::read_to_string(&log_path)
                .expect("fixture log loads")
                .matches("\"method\": \"turn/interrupt\"")
                .count()
        };
        let run_scenario = |scenario: &str,
                            scenario_policy: runtime::contracts::RunSupervisionPolicy,
                            cancellation: &RunCancellation| {
            let before = interrupt_count();
            let result = run_codex_turn_process_with_policy(
                "run-scenario",
                scenario,
                directory.path(),
                scenario,
                None,
                scenario_policy,
                &channel,
                cancellation,
            );
            assert!(
                interrupt_count().saturating_sub(before) <= 1,
                "scenario sent more than one interrupt: {scenario}"
            );
            result
        };
        for (scenario, expected) in [
            (
                "thread-hang",
                runtime::contracts::RunFailureCode::RequestTimeout,
            ),
            (
                "turn-hang",
                runtime::contracts::RunFailureCode::RequestTimeout,
            ),
            (
                "invalid-thread-id",
                runtime::contracts::RunFailureCode::ProtocolError,
            ),
            (
                "invalid-turn-id",
                runtime::contracts::RunFailureCode::ProtocolError,
            ),
            (
                "progress-hang",
                runtime::contracts::RunFailureCode::ProgressTimeout,
            ),
            (
                "terminal-hang",
                runtime::contracts::RunFailureCode::TerminalTimeout,
            ),
            (
                "malformed",
                runtime::contracts::RunFailureCode::ProtocolError,
            ),
            (
                "provider-error",
                runtime::contracts::RunFailureCode::ProviderError,
            ),
            (
                "terminal-failed",
                runtime::contracts::RunFailureCode::ProviderError,
            ),
            (
                "approval",
                runtime::contracts::RunFailureCode::PolicyViolation,
            ),
            (
                "child-exit",
                runtime::contracts::RunFailureCode::ChildExited,
            ),
            (
                "foreign",
                runtime::contracts::RunFailureCode::ProgressTimeout,
            ),
            (
                "response-too-large",
                runtime::contracts::RunFailureCode::ResponseTooLarge,
            ),
            (
                "stream-too-large",
                runtime::contracts::RunFailureCode::ResponseTooLarge,
            ),
        ] {
            let failure = run_scenario(scenario, policy, &RunCancellation::default())
                .expect_err("scenario must fail");
            assert_eq!(failure.code, expected, "scenario: {scenario}");
            assert!(!failure.message.contains("SAAA_PRIVATE_"));
        }
        let hard_policy = runtime::contracts::RunSupervisionPolicy {
            progress_idle_timeout_ms: 200,
            hard_timeout_ms: 30,
            ..policy
        };
        assert_eq!(
            run_scenario("hard-hang", hard_policy, &RunCancellation::default())
                .expect_err("hard timeout must fail")
                .code,
            runtime::contracts::RunFailureCode::HardTimeout
        );
        for forbidden in ["fileChange", "mcpToolCall", "dynamicToolCall", "webSearch"] {
            assert_eq!(
                run_scenario(forbidden, policy, &RunCancellation::default())
                    .expect_err("forbidden item must fail")
                    .code,
                runtime::contracts::RunFailureCode::PolicyViolation
            );
        }
        let duplicate = run_scenario("duplicate", policy, &RunCancellation::default())
            .expect("duplicate notifications must not break completion");
        assert_eq!(duplicate.content, "SAAA_DUPLICATE_OK");

        let unresponsive_cancel = Arc::new(RunCancellation::default());
        let cancel_trigger = unresponsive_cancel.clone();
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            cancel_trigger.cancel();
        });
        let unresponsive = run_scenario("cancel-no-response", policy, &unresponsive_cancel)
            .expect_err("unresponsive cancellation must finish");
        cancel_thread.join().expect("cancel trigger joins");
        assert_eq!(
            unresponsive.code,
            runtime::contracts::RunFailureCode::UserCancelled
        );
        let interrupts_before_atomic = interrupt_count();

        let mut connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
                 VALUES('coding-atomic','Atomic','coding','1','1')",
                [],
            )
            .expect("coding conversation inserts");
        let mut documents = default_settings_input();
        let codex = documents
            .iter_mut()
            .find(|document| document.namespace == "providers.agent")
            .expect("Codex settings exist");
        codex.value_json["enabled"] = Value::Bool(true);
        codex.value_json["model"] = Value::String("normal".to_string());
        save_settings_documents_to_connection(&mut connection, &documents)
            .expect("Codex settings save");
        let state = app_state(connection);
        let committed_terminal_states =
            Arc::new(Mutex::new(
                Vec::<(String, String, String, Option<String>)>::new(),
            ));
        let committed_terminal_states_for_channel = committed_terminal_states.clone();
        let database_for_channel = state.connection.clone();
        let atomic_channel: tauri::ipc::Channel<RuntimeEvent> =
            tauri::ipc::Channel::new(move |body| {
                if let tauri::ipc::InvokeResponseBody::Json(value) = body {
                    let event: Value = serde_json::from_str(&value).expect("runtime event decodes");
                    let event_type = event
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if matches!(event_type, "messageCompleted" | "cancelled" | "failed") {
                        let run_id = event
                            .get("runId")
                            .and_then(Value::as_str)
                            .expect("terminal event has run id");
                        let database = database_for_channel.lock().expect("database lock");
                        let (status, failure_code): (String, Option<String>) = database
                            .query_row(
                                "SELECT status,failure_code FROM runtime_runs WHERE id=?1",
                                [run_id],
                                |row| Ok((row.get(0)?, row.get(1)?)),
                            )
                            .expect("committed run reads from terminal callback");
                        committed_terminal_states_for_channel
                            .lock()
                            .expect("terminal state lock")
                            .push((
                                run_id.to_string(),
                                event_type.to_string(),
                                status,
                                failure_code,
                            ));
                    }
                }
                Ok(())
            });
        let atomic_input = StartTurnInput {
            run_id: "run-atomic".to_string(),
            conversation_id: "coding-atomic".to_string(),
            content: "ATOMIC".to_string(),
            workspace_path: Some(directory.path().to_string_lossy().into_owned()),
        };
        tauri::async_runtime::block_on(execute_turn(
            &state,
            &atomic_input,
            &atomic_channel,
            Arc::new(RunCancellation::default()),
            Some(policy),
        ))
        .expect("atomic Codex turn succeeds");
        assert_eq!(interrupt_count(), interrupts_before_atomic);
        assert_eq!(
            committed_terminal_states
                .lock()
                .expect("terminal state lock")
                .as_slice(),
            [(
                "run-atomic".to_string(),
                "messageCompleted".to_string(),
                "completed".to_string(),
                None
            )]
        );
        let database = state.connection.lock().expect("database lock");
        let (status, supervisor_version, assistant_count): (String, String, i64) = database
            .query_row(
                "SELECT r.status,r.supervisor_version,
                        (SELECT COUNT(*) FROM conversation_messages
                         WHERE conversation_id='coding-atomic' AND role='assistant')
                 FROM runtime_runs r WHERE r.id='run-atomic'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("atomic state reads");
        assert_eq!(status, "completed");
        assert_eq!(supervisor_version, runtime::contracts::SUPERVISOR_VERSION);
        assert_eq!(assistant_count, 1);
        drop(database);

        let run_atomic_scenario =
            |run_id: &str, scenario: &str, cancellation: Arc<RunCancellation>| {
                {
                    let mut database = state.connection.lock().expect("database lock");
                    let mut documents = default_settings_input();
                    let codex = documents
                        .iter_mut()
                        .find(|document| document.namespace == "providers.agent")
                        .expect("Codex settings exist");
                    codex.value_json["enabled"] = Value::Bool(true);
                    codex.value_json["model"] = Value::String(scenario.to_string());
                    save_settings_documents_to_connection(&mut database, &documents)
                        .expect("scenario Codex settings save");
                }
                let input = StartTurnInput {
                    run_id: run_id.to_string(),
                    conversation_id: "coding-atomic".to_string(),
                    content: scenario.to_string(),
                    workspace_path: Some(directory.path().to_string_lossy().into_owned()),
                };
                tauri::async_runtime::block_on(execute_turn(
                    &state,
                    &input,
                    &atomic_channel,
                    cancellation,
                    Some(policy),
                ))
            };

        let cancellation = Arc::new(RunCancellation::default());
        let cancel_trigger = cancellation.clone();
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            cancel_trigger.cancel();
        });
        let interrupts_before_cancel = interrupt_count();
        let cancelled =
            run_atomic_scenario("run-atomic-cancel", "cancel-no-response", cancellation)
                .expect_err("cancel scenario must cancel");
        cancel_thread.join().expect("atomic cancel trigger joins");
        assert!(interrupt_count().saturating_sub(interrupts_before_cancel) <= 1);
        assert_eq!(
            cancelled.code,
            runtime::contracts::RunFailureCode::UserCancelled
        );
        let interrupts_before_progress = interrupt_count();
        let progress = run_atomic_scenario(
            "run-atomic-progress",
            "progress-hang",
            Arc::new(RunCancellation::default()),
        )
        .expect_err("progress scenario must time out");
        assert_eq!(
            progress.code,
            runtime::contracts::RunFailureCode::ProgressTimeout
        );
        assert_eq!(interrupt_count() - interrupts_before_progress, 1);
        let interrupts_before_policy = interrupt_count();
        let policy_violation = run_atomic_scenario(
            "run-atomic-policy",
            "fileChange",
            Arc::new(RunCancellation::default()),
        )
        .expect_err("policy scenario must fail");
        assert_eq!(
            policy_violation.code,
            runtime::contracts::RunFailureCode::PolicyViolation
        );
        assert_eq!(interrupt_count() - interrupts_before_policy, 1);

        let terminal_states = committed_terminal_states
            .lock()
            .expect("terminal state lock");
        for expected in [
            (
                "run-atomic-cancel",
                "cancelled",
                "cancelled",
                Some("user-cancelled"),
            ),
            (
                "run-atomic-progress",
                "failed",
                "failed",
                Some("progress-timeout"),
            ),
            (
                "run-atomic-policy",
                "failed",
                "failed",
                Some("policy-violation"),
            ),
        ] {
            let matching = terminal_states
                .iter()
                .filter(|entry| entry.0 == expected.0)
                .collect::<Vec<_>>();
            assert_eq!(matching.len(), 1, "one terminal event for {}", expected.0);
            assert_eq!(matching[0].1, expected.1);
            assert_eq!(matching[0].2, expected.2);
            assert_eq!(matching[0].3.as_deref(), expected.3);
        }
        drop(terminal_states);
        if let Some(value) = previous {
            env::set_var("SAAA_CODEX_PATH", value);
        } else {
            env::remove_var("SAAA_CODEX_PATH");
        }
        assert!(cancellation_result.is_err());
        let log = fs::read_to_string(log_path).expect("fixture log loads");
        assert!(log.contains("thread/start"));
        assert!(log.contains("thread/resume"));
        assert!(log.contains("turn/interrupt"));
        assert!(log.contains("read-only"));
        assert!(log.contains("\"approvalPolicy\": \"never\""));
        assert!(log.contains("\"network_access\": false"));
        assert!(received
            .lock()
            .expect("event lock")
            .iter()
            .any(|event| { event.contains("SAAA_OK") && event.contains("\"type\":\"delta\"") }));
    }

    #[test]
    #[ignore = "requires a local Codex runtime and authentication"]
    fn codex_model_lookup_returns_visible_models() {
        let models = fetch_codex_models().expect("Codex models load");

        assert!(!models.is_empty());
        assert!(models.iter().all(|model| !model.hidden));
        assert!(models.iter().any(|model| model.is_default));
    }

    #[tokio::test]
    async fn larm_turn_commits_before_events_and_keeps_success_when_release_fails() {
        use std::ffi::OsString;
        use std::io::{Read, Write as _};
        use std::net::TcpListener;

        struct EnvGuard {
            key: &'static str,
            previous: Option<OsString>,
        }
        impl EnvGuard {
            fn set(key: &'static str, value: &str) -> Self {
                let previous = env::var_os(key);
                env::set_var(key, value);
                Self { key, previous }
            }
        }
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                if let Some(previous) = &self.previous {
                    env::set_var(self.key, previous);
                } else {
                    env::remove_var(self.key);
                }
            }
        }

        let _environment_lock = providers::larm::test_environment_lock().lock().await;
        let _token = EnvGuard::set("LARM_API_TOKEN", "fixture-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("fake LARM binds");
        let address = listener.local_addr().expect("fake LARM address");
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let server_requests = requests.clone();
        let allocation = concat!(
            "{\"id\":\"alloc_turn\",\"status\":\"ready\",",
            "\"requirements\":[{\"capability\":\"llm.general\",\"route\":\"llm-default\"}],",
            "\"bindings\":[{\"capability\":\"llm.general\",\"route\":\"llm-default\",",
            "\"runtime\":\"runtime_turn\",\"node\":\"gnosis\",\"status\":\"HOT\",",
            "\"candidateRank\":1,\"fallback\":false,\"selectionReason\":\"primary-live\"}],",
            "\"allowFallback\":false,\"deploymentPolicy\":\"existing-only\",",
            "\"createdAt\":\"2026-08-28T00:00:00.000Z\",",
            "\"expiresAt\":\"2026-08-28T00:05:00.000Z\"}"
        );
        let allocate_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{allocation}",
            allocation.len()
        );
        let tool_stream_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{",
            "\"index\":0,\"id\":\"call_larm_turn\",\"type\":\"function\",",
            "\"function\":{\"name\":\"recall_conversation\",",
            "\"arguments\":\"{\\\"query\\\":\\\"missing-history\\\"}\"}}]}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let tool_stream_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Request-ID: req_tool\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{tool_stream_body}",
            tool_stream_body.len()
        );
        let stream_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"LARM ok\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let stream_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Request-ID: req_turn\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{stream_body}",
            stream_body.len()
        );
        let release_error =
            r#"{"error":{"code":"internal_error","message":"fixture release failure"}}"#;
        let release_response = format!(
            "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{release_error}",
            release_error.len()
        );
        let server = thread::spawn(move || {
            for response in [
                allocate_response,
                tool_stream_response,
                stream_response,
                release_response,
            ] {
                let (mut socket, _) = listener.accept().expect("fake LARM accepts");
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .expect("fake LARM read timeout");
                let mut request = vec![0_u8; 64 * 1_024];
                let size = socket.read(&mut request).expect("fake LARM reads");
                server_requests
                    .lock()
                    .expect("request lock")
                    .push(String::from_utf8_lossy(&request[..size]).into_owned());
                socket
                    .write_all(response.as_bytes())
                    .expect("fake LARM writes");
            }
        });

        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut state = app_state(connection);
        state.larm_gate = providers::larm::LarmRuntimeGate::Ready(Arc::new(
            providers::larm::client::SharedLarmClient::build().expect("LARM client builds"),
        ));
        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings")
            .value_json = json!({ "providers": [{
                "kind": "larm", "id": "larm-primary", "enabled": true, "label": "LARM",
                "location": "local", "baseUrl": format!("http://{address}"),
                "tokenEnv": "LARM_API_TOKEN", "allocationTtlSeconds": 300,
                "allocationStartupTimeoutSeconds": 5, "allowFallbackByDefault": false,
                "deploymentPolicy": "existing-only"
            }] });
        let routing = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        routing.value_json["conversationRespond"]["primaryProviderId"] = json!("larm-primary");
        routing.value_json["conversationRespond"]["fallbackProviderIds"] = json!([]);
        save_settings_documents_to_connection(
            &mut state.connection.lock().expect("database lock"),
            &documents,
        )
        .expect("settings save");

        let event_states = Arc::new(Mutex::new(Vec::<String>::new()));
        let callback_states = event_states.clone();
        let callback_database = state.connection.clone();
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
            let tauri::ipc::InvokeResponseBody::Json(event) = body else {
                return Ok(());
            };
            if event.contains("\"type\":\"providerSelected\"") {
                let selected: (Option<String>, i64, String) = callback_database
                    .lock()
                    .expect("database lock")
                    .query_row(
                        "SELECT selected_runtime_id, output_started, status
                         FROM provider_sessions WHERE runtime_run_id='run-larm'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .expect("selection row");
                callback_states.lock().expect("state lock").push(format!(
                    "selected:{:?}:{}:{}",
                    selected.0, selected.1, selected.2
                ));
            } else if event.contains("\"type\":\"delta\"") {
                let output_started: i64 = callback_database
                    .lock()
                    .expect("database lock")
                    .query_row(
                        "SELECT output_started FROM provider_sessions WHERE runtime_run_id='run-larm'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("output row");
                callback_states
                    .lock()
                    .expect("state lock")
                    .push(format!("delta:{output_started}"));
            } else if event.contains("\"type\":\"messageCompleted\"") {
                let terminal: (String, String, String) = callback_database
                    .lock()
                    .expect("database lock")
                    .query_row(
                        "SELECT ps.status, ps.release_status, rr.status
                         FROM provider_sessions ps JOIN runtime_runs rr ON rr.id=ps.runtime_run_id
                         WHERE ps.runtime_run_id='run-larm'",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .expect("terminal rows");
                callback_states.lock().expect("state lock").push(format!(
                    "completed:{}:{}:{}",
                    terminal.0, terminal.1, terminal.2
                ));
            }
            Ok(())
        });
        let input = StartTurnInput {
            run_id: "run-larm".to_string(),
            conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
            content: "fixture prompt".to_string(),
            workspace_path: None,
        };
        execute_turn(
            &state,
            &input,
            &channel,
            Arc::new(RunCancellation::default()),
            None,
        )
        .await
        .expect("LARM turn completes");
        server.join().expect("fake LARM joins");

        assert_eq!(
            *event_states.lock().expect("state lock"),
            vec![
                "selected:Some(\"runtime_turn\"):0:running",
                "delta:1",
                "completed:completed:deferred-to-ttl:completed"
            ]
        );
        let captures = requests.lock().expect("request lock");
        assert_eq!(captures.len(), 4);
        assert!(captures.iter().all(|request| request
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-token")));
        assert!(captures[1].contains("x-larm-allocation-id: alloc_turn"));
        assert!(captures[1].contains("\"name\":\"recall_conversation\""));
        assert!(captures[2].contains("\"role\":\"tool\""));
        assert!(captures[2].contains("continuity-no-hit"));
        let telemetry: (String, String, i64, String, String, Option<String>, String) = state
            .connection
            .lock()
            .expect("database lock")
            .query_row(
                "SELECT route_id, selected_runtime_id, fallback_used, selection_reason,
                        release_status, request_id, release_failure_kind
                 FROM provider_sessions WHERE runtime_run_id='run-larm'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .expect("telemetry row");
        assert_eq!(
            telemetry,
            (
                "llm-default".to_string(),
                "runtime_turn".to_string(),
                0,
                "primary".to_string(),
                "deferred-to-ttl".to_string(),
                Some("req_turn".to_string()),
                "upstream".to_string()
            )
        );
    }

    #[test]
    #[ignore = "requires a local Codex runtime, authentication, and network access"]
    fn codex_live_read_only_turn_completes() {
        let status = fetch_codex_status().expect("Codex status loads");
        assert!(status.installed);
        assert!(status.authenticated, "{}", status.message);
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
        let outcome = run_codex_turn_process(
            "run-live-smoke",
            "Reply with exactly SAAA_LIVE_OK. Do not use tools.",
            workspace.path(),
            "",
            None,
            120_000,
            &events,
            &RunCancellation::default(),
        )
        .expect("live Codex turn succeeds");
        assert!(outcome.content.contains("SAAA_LIVE_OK"));
        assert_eq!(
            fs::read_dir(workspace.path())
                .expect("workspace remains readable")
                .count(),
            0,
            "read-only Codex turn must not create workspace files"
        );
    }

    #[test]
    #[ignore = "requires a local Codex runtime, authentication, and network access"]
    fn codex_live_read_only_turn_cancels_after_turn_start() {
        let status = fetch_codex_status().expect("Codex status loads");
        assert!(status.installed);
        assert!(status.authenticated, "{}", status.message);
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let cancellation = Arc::new(RunCancellation::default());
        let cancellation_for_events = cancellation.clone();
        let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |_| {
            cancellation_for_events.cancel();
            Ok(())
        });
        let failure = run_codex_turn_process(
            "run-live-cancel",
            "Explain the read-only runtime lifecycle in detail. Do not use tools.",
            workspace.path(),
            "",
            None,
            120_000,
            &events,
            &cancellation,
        )
        .expect_err("live Codex turn is cancelled after turn/start");
        assert_eq!(
            failure.code,
            runtime::contracts::RunFailureCode::UserCancelled
        );
        assert_eq!(
            fs::read_dir(workspace.path())
                .expect("workspace remains readable")
                .count(),
            0,
            "cancelled read-only Codex turn must not create workspace files"
        );
    }
}
