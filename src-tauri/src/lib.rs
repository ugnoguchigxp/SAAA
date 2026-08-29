use futures_util::StreamExt;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{hash_map::Entry, HashMap},
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
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

mod meeting;
mod memory;
mod providers;
mod runtime;
mod situation;
mod voice;

static BUNDLED_CODEX_PATH: OnceLock<PathBuf> = OnceLock::new();
const MAX_CODEX_STDOUT_BYTES: u64 = 4 * 1_024 * 1_024;
const WINDOW_SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const GNOSIS_PROVIDER_ID: &str = "gnosis-qwen";
const GNOSIS_ENDPOINT: &str = "http://192.168.0.65:8080/v1";
const GNOSIS_MODEL: &str = "Qwen3.8-27B-ROCmFP4-FAST.gguf";
const PRIMARY_CONVERSATION_ID: &str = "conversation_primary";
const PRIMARY_CONVERSATION_TITLE: &str = "SAAAとの会話";
const CODEX_READ_ONLY_SYSTEM_CONTEXT: &str = include_str!("../../.s11tnext/codex-read-only.txt");

struct AppState {
    connection: Arc<Mutex<Connection>>,
    active_runs: Mutex<HashMap<String, Arc<RunCancellation>>>,
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

pub(crate) struct ProcessGuard {
    child: Child,
    terminated: bool,
}

impl ProcessGuard {
    pub(crate) fn new(child: Child) -> Self {
        Self {
            child,
            terminated: false,
        }
    }

    pub(crate) fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    pub(crate) fn terminate(&mut self) {
        if self.terminated {
            return;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.terminated = true;
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.terminate();
    }
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationMessage {
    id: String,
    conversation_id: String,
    role: String,
    content: String,
    created_at: String,
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
struct OpenAiCompatibleProviderSettings {
    id: String,
    enabled: bool,
    label: String,
    location: String,
    endpoint: String,
    model: String,
    credential_status: String,
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
#[serde(tag = "kind", deny_unknown_fields)]
enum ModelProviderSettings {
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible(OpenAiCompatibleProviderSettings),
    #[serde(rename = "larm")]
    Larm(LarmProviderSettings),
}

impl ModelProviderSettings {
    fn id(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.id,
            Self::Larm(provider) => &provider.id,
        }
    }

    fn enabled(&self) -> bool {
        match self {
            Self::OpenAiCompatible(provider) => provider.enabled,
            Self::Larm(provider) => provider.enabled,
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.label,
            Self::Larm(provider) => &provider.label,
        }
    }

    fn location(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.location,
            Self::Larm(provider) => &provider.location,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible(_) => "openai-compatible",
            Self::Larm(_) => "larm",
        }
    }

    fn set_enabled(&mut self, enabled: bool) {
        match self {
            Self::OpenAiCompatible(provider) => provider.enabled = enabled,
            Self::Larm(provider) => provider.enabled = enabled,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelProvidersSettings {
    providers: Vec<ModelProviderSettings>,
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

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum RuntimeEvent {
    Started {
        run_id: String,
        route: String,
        provider_id: String,
    },
    ProviderSelected {
        run_id: String,
        provider_id: String,
        provider_kind: String,
        route_id: String,
        runtime_id: String,
        fallback_used: bool,
        selection_reason_code: String,
    },
    Delta {
        run_id: String,
        text: String,
    },
    Activity {
        run_id: String,
        kind: String,
        summary: String,
    },
    ProviderFailed {
        run_id: String,
        provider_id: String,
        reason: String,
    },
    MessageCompleted {
        run_id: String,
        message: ConversationMessage,
    },
    Cancelled {
        run_id: String,
    },
    Failed {
        run_id: String,
        code: String,
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
fn export_diagnostics(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<LocalArtifactResult, String> {
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
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not resolve the diagnostics directory: {error}"))?
        .join("diagnostics");
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

fn build_provider_diagnostics(connection: &Connection) -> Result<Value, String> {
    let mut provider_statement = connection
        .prepare(
            "SELECT COALESCE(provider_kind, 'openai-compatible'), COALESCE(route_id, ''),
                    COALESCE(selected_runtime_id, ''), COALESCE(fallback_used, 0),
                    COALESCE(selection_reason, ''), status, COALESCE(failure_kind, ''),
                    release_status, COALESCE(release_failure_kind, '')
             FROM provider_sessions ORDER BY updated_at DESC LIMIT 20",
        )
        .map_err(database_error)?;
    let recent = provider_statement
        .query_map([], |row| {
            Ok(json!({
                "providerKind": row.get::<_, String>(0)?,
                "route": row.get::<_, String>(1)?,
                "runtime": row.get::<_, String>(2)?,
                "fallbackUsed": row.get::<_, bool>(3)?,
                "selectionReason": row.get::<_, String>(4)?,
                "status": row.get::<_, String>(5)?,
                "failureKind": row.get::<_, String>(6)?,
                "releaseStatus": row.get::<_, String>(7)?,
                "releaseFailureKind": row.get::<_, String>(8)?
            }))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(provider_statement);
    let mut aggregate_statement = connection
        .prepare(
            "SELECT COALESCE(provider_kind, 'openai-compatible'), COALESCE(route_id, ''),
                    COALESCE(selected_runtime_id, ''), COALESCE(fallback_used, 0),
                    COALESCE(failure_kind, ''), release_status, COUNT(*)
             FROM provider_sessions
             GROUP BY provider_kind, route_id, selected_runtime_id, fallback_used,
                      failure_kind, release_status
             ORDER BY COUNT(*) DESC LIMIT 50",
        )
        .map_err(database_error)?;
    let aggregates = aggregate_statement
        .query_map([], |row| {
            Ok(json!({
                "providerKind": row.get::<_, String>(0)?,
                "route": row.get::<_, String>(1)?,
                "runtime": row.get::<_, String>(2)?,
                "fallbackUsed": row.get::<_, bool>(3)?,
                "failureKind": row.get::<_, String>(4)?,
                "releaseStatus": row.get::<_, String>(5)?,
                "count": row.get::<_, i64>(6)?
            }))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(json!({ "recent": recent, "aggregates": aggregates }))
}

#[tauri::command]
fn backup_database(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<LocalArtifactResult, String> {
    let created_at = now_iso();
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Could not resolve the backup directory: {error}"))?
        .join("backups");
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
    let tts_process = state
        .tts_process
        .lock()
        .map_err(|_| "TTS process lock unavailable".to_string())?;
    if tts_process.is_some() {
        return Err("MEETING_POLICY_TTS_BLOCKED: Stop speech and retry.".to_string());
    }
    let snapshot = state.meeting.start(&input, &state.connection)?;
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

async fn execute_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: Arc<RunCancellation>,
    codex_policy_override: Option<runtime::contracts::RunSupervisionPolicy>,
) -> Result<(), TurnExecutionFailure> {
    let task_mode = match prepare_runtime_run(state, input) {
        Ok(task_mode) => task_mode,
        Err(message) => {
            let _ = on_event.send(RuntimeEvent::Failed {
                run_id: input.run_id.clone(),
                code: "runtime_error".to_string(),
                message: redact_runtime_text(&message),
                recovery: "Review the conversation and runtime state, then retry.".to_string(),
            });
            return Err(TurnExecutionFailure::unsupervised(
                runtime::contracts::RunFailureCode::InternalError,
                message,
            ));
        }
    };
    state
        .situation
        .set_conversation_state(if task_mode == "coding" {
            situation::contracts::ConversationState::AgentRunning
        } else {
            situation::contracts::ConversationState::ModelRunning
        });
    if task_mode == "coding" {
        let result = execute_codex_turn(
            state,
            input,
            on_event,
            cancellation.clone(),
            codex_policy_override,
        )
        .await;
        if let Err(error) = &result {
            if !error.finalized {
                let cancelled = cancellation.is_cancelled()
                    || error.code == runtime::contracts::RunFailureCode::UserCancelled;
                finish_supervised_runtime_run(
                    state,
                    &input.run_id,
                    if cancelled { "cancelled" } else { "failed" },
                    Some(if cancelled {
                        runtime::contracts::RunFailureCode::UserCancelled
                    } else {
                        error.code
                    }),
                    error.supervisor_version,
                    error.last_progress_at.as_deref(),
                    Some(&error.message),
                )
                .map_err(|message| {
                    TurnExecutionFailure::unsupervised(
                        runtime::contracts::RunFailureCode::InternalError,
                        message,
                    )
                })?;
                send_runtime_terminal_event(on_event, &input.run_id, error, cancelled);
            }
        }
        state
            .situation
            .set_conversation_state(situation::contracts::ConversationState::Idle);
        return result.map(|_| ());
    }

    let result = execute_conversation_turn(state, input, on_event, cancellation.clone())
        .await
        .map_err(|message| {
            TurnExecutionFailure::unsupervised(
                runtime::contracts::RunFailureCode::ProviderError,
                message,
            )
        });
    let finalization = match &result {
        Ok(message) => {
            let _ = on_event.send(RuntimeEvent::MessageCompleted {
                run_id: input.run_id.clone(),
                message: message.clone(),
            });
            Ok(())
        }
        Err(error) if cancellation.is_cancelled() => {
            let finalization = finish_supervised_runtime_run(
                state,
                &input.run_id,
                "cancelled",
                Some(runtime::contracts::RunFailureCode::UserCancelled),
                None,
                None,
                Some("Cancelled by user"),
            );
            if finalization.is_ok() {
                let _ = on_event.send(RuntimeEvent::Cancelled {
                    run_id: input.run_id.clone(),
                });
            }
            finalization
        }
        Err(error) => {
            let finalization = finish_supervised_runtime_run(
                state,
                &input.run_id,
                "failed",
                None,
                None,
                None,
                Some(&error.message),
            );
            if finalization.is_ok() {
                let _ = on_event.send(RuntimeEvent::Failed {
                    run_id: input.run_id.clone(),
                    code: "runtime_error".to_string(),
                    message: redact_runtime_text(&error.message),
                    recovery: "Review the selected provider and runtime settings, then retry."
                        .to_string(),
                });
            }
            finalization
        }
    };
    state
        .situation
        .set_conversation_state(situation::contracts::ConversationState::Idle);
    finalization.map_err(|message| {
        TurnExecutionFailure::unsupervised(
            runtime::contracts::RunFailureCode::InternalError,
            message,
        )
    })?;
    result.map(|_| ())
}

fn send_runtime_terminal_event(
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    run_id: &str,
    error: &TurnExecutionFailure,
    cancelled: bool,
) {
    if cancelled {
        let _ = on_event.send(RuntimeEvent::Cancelled {
            run_id: run_id.to_string(),
        });
    } else {
        let _ = on_event.send(RuntimeEvent::Failed {
            run_id: run_id.to_string(),
            code: error.code.as_str().to_string(),
            message: redact_runtime_text(&error.message),
            recovery: error.code.recovery().to_string(),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_supervised_runtime_run(
    state: &AppState,
    run_id: &str,
    status: &str,
    failure_code: Option<runtime::contracts::RunFailureCode>,
    supervisor_version: Option<&str>,
    last_progress_at: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE runtime_runs
             SET status=?1, error_message=?2, completed_at=?3, failure_code=?4,
                 supervisor_version=?5, last_progress_at=?6
             WHERE id=?7 AND status='running'",
            params![
                status,
                error.map(redact_runtime_text),
                now_iso(),
                failure_code.map(runtime::contracts::RunFailureCode::as_str),
                supervisor_version,
                last_progress_at,
                run_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Runtime run was already finalized".to_string());
    }
    Ok(())
}

fn prepare_runtime_run(state: &AppState, input: &StartTurnInput) -> Result<String, String> {
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let transaction = connection.transaction().map_err(database_error)?;
    let task_mode: String = transaction
        .query_row(
            "SELECT task_mode FROM conversations WHERE id = ?1",
            params![input.conversation_id],
            |row| row.get(0),
        )
        .map_err(|_| "Conversation does not exist".to_string())?;
    let route_kind = if task_mode == "coding" {
        "coding.assist"
    } else {
        "conversation.respond"
    };
    let now = now_iso();
    let input_message_id = new_id("message");
    transaction
        .execute(
            "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
             VALUES (?1, ?2, 'user', ?3, ?4)",
            params![
                input_message_id,
                input.conversation_id,
                input.content.trim(),
                now
            ],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO runtime_runs(
               id,conversation_id,route_kind,status,started_at,supervisor_version,input_message_id
             ) VALUES(?1,?2,?3,'running',?4,?5,?6)",
            params![
                input.run_id,
                input.conversation_id,
                route_kind,
                now,
                if task_mode == "coding" {
                    Some(runtime::contracts::SUPERVISOR_VERSION)
                } else {
                    None
                },
                input_message_id,
            ],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "UPDATE conversations SET updated_at = ?1, title = COALESCE(title, ?2) WHERE id = ?3",
            params![
                now,
                bounded_text(input.content.trim(), 60),
                input.conversation_id
            ],
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)?;
    Ok(task_mode)
}

fn finish_runtime_run(
    state: &AppState,
    run_id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE runtime_runs
             SET status = ?1, error_message = ?2, completed_at = ?3
             WHERE id = ?4 AND status = 'running'",
            params![status, error.map(redact_runtime_text), now_iso(), run_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Runtime run was already finalized".to_string());
    }
    Ok(())
}

async fn execute_conversation_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: Arc<RunCancellation>,
) -> Result<ConversationMessage, String> {
    let (providers, route, security, history, context_health) = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        let input_message_id: String = connection
            .query_row(
                "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                params![input.run_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let context_window =
            memory::context_window::build(&connection, &input.conversation_id, &input_message_id)?;
        let context_health = context_window.health.clone();
        let history = context_window
            .messages
            .into_iter()
            .enumerate()
            .map(|(index, message)| ConversationMessage {
                id: format!("context-projection-{index}"),
                conversation_id: input.conversation_id.clone(),
                role: message.role,
                content: message.content,
                created_at: index.to_string(),
            })
            .collect::<Vec<_>>();
        (
            load_model_providers(&connection)?,
            load_routing_settings(&connection)?.conversation_respond,
            load_security_settings(&connection)?,
            history,
            context_health,
        )
    };
    let route_ids = apply_runtime_provider_gates(
        &providers,
        effective_conversation_route_ids(&providers, &route, &security),
        &state.larm_gate,
    );
    if route_ids.is_empty() && !state.larm_gate.allows_traffic() {
        return Err(state.larm_gate.public_message().to_string());
    }
    let mut failures = Vec::new();
    let mut context_health_emitted = false;

    for provider_id in route_ids {
        if cancellation.is_cancelled() {
            return Err("Cancelled by user".to_string());
        }
        let Some(provider) = providers
            .providers
            .iter()
            .find(|provider| provider.id() == provider_id && provider.enabled())
            .cloned()
        else {
            failures.push(format!("{provider_id}: provider is disabled or missing"));
            continue;
        };
        update_runtime_provider(state, &input.run_id, provider.id())?;
        let session_id =
            begin_provider_session(state, &input.run_id, provider.id(), provider.kind())?;
        if on_event
            .send(RuntimeEvent::Started {
                run_id: input.run_id.clone(),
                route: "conversation.respond".to_string(),
                provider_id: provider.id().to_string(),
            })
            .is_err()
        {
            if provider.kind() == "larm" {
                finish_larm_provider_session(
                    state,
                    &session_id,
                    "failed",
                    Some(ProviderFailureKind::ClientDisconnected),
                    CleanupOutcome::NotStarted,
                )?;
            } else {
                finish_provider_session(
                    state,
                    &session_id,
                    "failed",
                    Some(ProviderFailureKind::ClientDisconnected),
                )?;
            }
            return Err(ProviderFailureKind::ClientDisconnected
                .public_message()
                .as_str()
                .to_string());
        }
        if !context_health_emitted {
            let _ = on_event.send(RuntimeEvent::Activity {
                run_id: input.run_id.clone(),
                kind: "context-window".to_string(),
                summary: format!(
                    "Context {}: {}/{} bytes, {} recent messages, {} continuity groups, {} source messages omitted",
                    context_health.status,
                    context_health.projected_bytes,
                    context_health.hard_limit_bytes,
                    context_health.recent_source_messages,
                    context_health.continuity_group_count,
                    context_health.dropped_source_messages,
                ),
            });
            context_health_emitted = true;
        }
        let outcome = match &provider {
            ModelProviderSettings::OpenAiCompatible(provider) => {
                stream_model_provider(
                    provider,
                    &history,
                    route.timeout_ms,
                    input,
                    on_event,
                    cancellation.clone(),
                    Some(ProviderOutputPersistence {
                        state,
                        session_id: &session_id,
                    }),
                )
                .await
            }
            ModelProviderSettings::Larm(provider) => {
                stream_larm_provider(
                    provider,
                    &history,
                    route.timeout_ms,
                    cancellation.clone(),
                    LarmStreamContext {
                        state,
                        session_id: &session_id,
                        input,
                        on_event,
                    },
                )
                .await
            }
        };
        match outcome {
            ProviderAttemptOutcome::Completed { content, cleanup } => {
                if provider.kind() == "larm" {
                    finish_larm_provider_session(state, &session_id, "completed", None, cleanup)?;
                } else {
                    finish_provider_session(state, &session_id, "completed", None)?;
                }
                return persist_conversation_success(state, input, &content);
            }
            ProviderAttemptOutcome::Cancelled { cleanup, .. } => {
                if provider.kind() == "larm" {
                    finish_larm_provider_session(
                        state,
                        &session_id,
                        "cancelled",
                        Some(ProviderFailureKind::Cancelled),
                        cleanup,
                    )?;
                } else {
                    finish_provider_session(
                        state,
                        &session_id,
                        "cancelled",
                        Some(ProviderFailureKind::Cancelled),
                    )?;
                }
                return Err("Cancelled by user".to_string());
            }
            ProviderAttemptOutcome::Failed {
                kind,
                public_message,
                output_started,
                cleanup,
            } => {
                let reason = public_message.as_str();
                if provider.kind() == "larm" {
                    finish_larm_provider_session(
                        state,
                        &session_id,
                        "failed",
                        Some(kind),
                        cleanup,
                    )?;
                } else {
                    finish_provider_session(state, &session_id, "failed", Some(kind))?;
                }
                let _ = on_event.send(RuntimeEvent::ProviderFailed {
                    run_id: input.run_id.clone(),
                    provider_id: provider.id().to_string(),
                    reason: reason.to_string(),
                });
                let failure = format!("{}: {reason}", provider.id());
                if !provider_fallback_allowed(kind, output_started) {
                    return Err(failure);
                }
                failures.push(failure);
            }
        }
    }
    Err(format!(
        "All configured providers failed. {}",
        failures.join("; ")
    ))
}

fn apply_runtime_provider_gates(
    providers: &ModelProvidersSettings,
    route_ids: Vec<String>,
    larm_gate: &providers::larm::LarmRuntimeGate,
) -> Vec<String> {
    route_ids
        .into_iter()
        .filter(|provider_id| {
            providers
                .providers
                .iter()
                .find(|provider| provider.id() == provider_id)
                .is_none_or(|provider| provider.kind() != "larm" || larm_gate.allows_traffic())
        })
        .collect()
}

async fn execute_codex_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: Arc<RunCancellation>,
    policy_override: Option<runtime::contracts::RunSupervisionPolicy>,
) -> Result<TurnCompletion, TurnExecutionFailure> {
    let workspace = input
        .workspace_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            TurnExecutionFailure::configuration("Select a workspace before starting a Codex turn")
        })?;
    let workspace = fs::canonicalize(workspace).map_err(|_| {
        TurnExecutionFailure::configuration("The selected Codex workspace does not exist")
    })?;
    if !workspace.is_dir() {
        return Err(TurnExecutionFailure::configuration(
            "The selected Codex workspace is not a directory",
        ));
    }
    let (settings, timeout_ms, existing_thread_id) = {
        let connection = state
            .connection
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        let settings = load_codex_settings(&connection)?;
        let routing = load_routing_settings(&connection)?;
        let thread_id = connection
            .query_row(
                "SELECT thread_id FROM codex_threads WHERE conversation_id = ?1 AND workspace_path = ?2",
                params![input.conversation_id, workspace.to_string_lossy()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(database_error)?;
        (settings, routing.coding_assist.timeout_ms, thread_id)
    };
    if !settings.enabled {
        return Err(TurnExecutionFailure::configuration(
            "Codex is disabled in Settings",
        ));
    }
    update_runtime_provider(state, &input.run_id, "codex-sdk")?;
    let run_id = input.run_id.clone();
    let prompt = input.content.clone();
    let model = settings.model.clone();
    let workspace_for_worker = workspace.clone();
    let on_event_for_worker = on_event.clone();
    let cancellation_for_worker = cancellation.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        if let Some(policy) = policy_override {
            run_codex_turn_process_with_policy(
                &run_id,
                &prompt,
                &workspace_for_worker,
                &model,
                existing_thread_id.as_deref(),
                policy,
                &on_event_for_worker,
                &cancellation_for_worker,
            )
        } else {
            run_codex_turn_process(
                &run_id,
                &prompt,
                &workspace_for_worker,
                &model,
                existing_thread_id.as_deref(),
                timeout_ms,
                &on_event_for_worker,
                &cancellation_for_worker,
            )
        }
    })
    .await
    .map_err(|error| format!("Codex runtime task failed: {error}"))?;

    match outcome {
        Ok(outcome) => {
            let message =
                persist_codex_success(state, input, &outcome, &settings.model, &workspace)?;
            let _ = on_event.send(RuntimeEvent::MessageCompleted {
                run_id: input.run_id.clone(),
                message,
            });
            Ok(TurnCompletion)
        }
        Err(failure) => {
            let cancelled = failure.code == runtime::contracts::RunFailureCode::UserCancelled;
            let mut error = TurnExecutionFailure {
                code: failure.code,
                message: failure.message,
                supervisor_version: Some(runtime::contracts::SUPERVISOR_VERSION),
                last_progress_at: failure.last_progress_at,
                finalized: false,
            };
            persist_codex_failure(
                state,
                input,
                failure.thread_id.as_deref(),
                &settings.model,
                &workspace,
                &error,
                cancelled,
            )?;
            error.finalized = true;
            send_runtime_terminal_event(on_event, &input.run_id, &error, cancelled);
            Err(error)
        }
    }
}

fn load_codex_settings(connection: &Connection) -> Result<CodexAgentRuntimeSettings, String> {
    let document = read_settings_document(connection, "providers.agent", "codex-sdk")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode Codex settings: {error}"))?;
    validate_codex_settings(&settings)?;
    Ok(settings)
}

fn upsert_codex_thread(
    transaction: &rusqlite::Transaction<'_>,
    conversation_id: &str,
    thread_id: &str,
    model: &str,
    workspace: &str,
) -> Result<(), String> {
    transaction
        .execute(
            "INSERT INTO codex_threads(conversation_id, thread_id, model, workspace_path, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(conversation_id) DO UPDATE SET
               thread_id = excluded.thread_id,
               model = excluded.model,
               workspace_path = excluded.workspace_path,
               updated_at = excluded.updated_at",
            params![conversation_id, thread_id, model, workspace, now_iso()],
        )
        .map_err(database_error)?;
    Ok(())
}

#[cfg(test)]
fn persist_codex_thread(
    state: &AppState,
    conversation_id: &str,
    thread_id: &str,
    model: &str,
    workspace: &std::path::Path,
) -> Result<(), String> {
    let workspace = workspace
        .to_str()
        .ok_or_else(|| "Codex workspace path is not valid UTF-8".to_string())?;
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let transaction = connection.transaction().map_err(database_error)?;
    upsert_codex_thread(&transaction, conversation_id, thread_id, model, workspace)?;
    transaction.commit().map_err(database_error)
}

fn persist_codex_success(
    state: &AppState,
    input: &StartTurnInput,
    outcome: &CodexTurnOutcome,
    model: &str,
    workspace: &std::path::Path,
) -> Result<ConversationMessage, String> {
    let workspace = workspace
        .to_str()
        .ok_or_else(|| "Codex workspace path is not valid UTF-8".to_string())?;
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let transaction = connection.transaction().map_err(database_error)?;
    upsert_codex_thread(
        &transaction,
        &input.conversation_id,
        &outcome.thread_id,
        model,
        workspace,
    )?;
    let message = ConversationMessage {
        id: new_id("message"),
        conversation_id: input.conversation_id.clone(),
        role: "assistant".to_string(),
        content: outcome.content.clone(),
        created_at: now_iso(),
    };
    transaction
        .execute(
            "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES(?1,?2,'assistant',?3,?4)",
            params![
                message.id,
                message.conversation_id,
                message.content,
                message.created_at
            ],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "UPDATE conversations SET updated_at=?1 WHERE id=?2",
            params![message.created_at, input.conversation_id],
        )
        .map_err(database_error)?;
    let changed = transaction
        .execute(
            "UPDATE runtime_runs
             SET status='completed',error_message=NULL,completed_at=?1,failure_code=NULL,
                 supervisor_version=?2,last_progress_at=?3
             WHERE id=?4 AND status='running'",
            params![
                now_iso(),
                runtime::contracts::SUPERVISOR_VERSION,
                outcome.last_progress_at,
                input.run_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Runtime run was already finalized".to_string());
    }
    transaction.commit().map_err(database_error)?;
    Ok(message)
}

fn persist_codex_failure(
    state: &AppState,
    input: &StartTurnInput,
    thread_id: Option<&str>,
    model: &str,
    workspace: &std::path::Path,
    error: &TurnExecutionFailure,
    cancelled: bool,
) -> Result<(), String> {
    let workspace = workspace
        .to_str()
        .ok_or_else(|| "Codex workspace path is not valid UTF-8".to_string())?;
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let transaction = connection.transaction().map_err(database_error)?;
    if let Some(thread_id) = thread_id {
        upsert_codex_thread(
            &transaction,
            &input.conversation_id,
            thread_id,
            model,
            workspace,
        )?;
    }
    let changed = transaction
        .execute(
            "UPDATE runtime_runs
             SET status=?1,error_message=?2,completed_at=?3,failure_code=?4,
                 supervisor_version=?5,last_progress_at=?6
             WHERE id=?7 AND status='running'",
            params![
                if cancelled { "cancelled" } else { "failed" },
                redact_runtime_text(&error.message),
                now_iso(),
                error.code.as_str(),
                runtime::contracts::SUPERVISOR_VERSION,
                error.last_progress_at,
                input.run_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Runtime run was already finalized".to_string());
    }
    transaction.commit().map_err(database_error)
}

#[allow(clippy::too_many_arguments)]
fn run_codex_turn_process(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    timeout_ms: u64,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: &RunCancellation,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    let policy =
        runtime::contracts::RunSupervisionPolicy::for_route(timeout_ms).map_err(|message| {
            CodexTurnFailure {
                thread_id: existing_thread_id.map(str::to_string),
                message,
                code: runtime::contracts::RunFailureCode::ConfigurationError,
                last_progress_at: None,
            }
        })?;
    run_codex_turn_process_with_policy(
        run_id,
        prompt,
        workspace,
        model,
        existing_thread_id,
        policy,
        on_event,
        cancellation,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_codex_turn_process_with_policy(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    policy: runtime::contracts::RunSupervisionPolicy,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: &RunCancellation,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    use runtime::codex_app_server::{CodexEventProjector, ProjectedCodexEvent};
    use runtime::contracts::{RunFailureCode, RunOutcome, RunSignal, TerminalStatus};
    use runtime::supervisor::RunSupervisor;

    let process_started = std::time::Instant::now();
    let mut supervisor = RunSupervisor::new(policy, 0);
    let mut child =
        ProcessGuard::new(
            spawn_codex_app_server().map_err(|message| CodexTurnFailure {
                thread_id: existing_thread_id.map(str::to_string),
                message,
                code: RunFailureCode::ChildStartFailed,
                last_progress_at: None,
            })?,
        );
    let mut stdin = child
        .child_mut()
        .stdin
        .take()
        .ok_or_else(|| CodexTurnFailure {
            thread_id: existing_thread_id.map(str::to_string),
            message: "Codex app-server stdin is unavailable".to_string(),
            code: RunFailureCode::ChildStartFailed,
            last_progress_at: None,
        })?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| CodexTurnFailure {
            thread_id: existing_thread_id.map(str::to_string),
            message: "Codex app-server stdout is unavailable".to_string(),
            code: RunFailureCode::ChildStartFailed,
            last_progress_at: None,
        })?;
    let (sender, receiver) = mpsc::sync_channel(256);
    let stdout_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout.take(MAX_CODEX_STDOUT_BYTES + 1));
        let mut bytes_read = 0_u64;
        loop {
            let mut line = String::new();
            let count = match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(count) => count,
                Err(_) => {
                    let _ = sender.send(CodexReaderMessage::Failed {
                        code: RunFailureCode::ProtocolError,
                        message: "Could not read Codex app-server output",
                    });
                    break;
                }
            };
            bytes_read = bytes_read.saturating_add(count as u64);
            if bytes_read > MAX_CODEX_STDOUT_BYTES {
                let _ = sender.send(CodexReaderMessage::Failed {
                    code: RunFailureCode::ResponseTooLarge,
                    message: "Codex app-server output exceeded the bounded stream limit",
                });
                break;
            }
            match serde_json::from_str::<Value>(line.trim_end()) {
                Ok(message) => {
                    if sender.send(CodexReaderMessage::Message(message)).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = sender.send(CodexReaderMessage::Failed {
                        code: RunFailureCode::ProtocolError,
                        message: "Codex app-server returned invalid JSON",
                    });
                    break;
                }
            }
        }
    });
    let mut thread_id = existing_thread_id.map(str::to_string);
    let mut last_progress_at = None;
    let result = (|| {
        if thread_id
            .as_deref()
            .is_some_and(|id| validate_identifier(id, "Codex thread id").is_err())
        {
            return Err(CodexTurnFailure {
                thread_id: None,
                message: "Persisted Codex thread id is invalid".to_string(),
                code: RunFailureCode::ProtocolError,
                last_progress_at: None,
            });
        }
        write_codex_handshake(&mut stdin).map_err(|message| CodexTurnFailure {
            thread_id: thread_id.clone(),
            code: RunFailureCode::ChildExited,
            message,
            last_progress_at: None,
        })?;
        let workspace_text = workspace.to_str().ok_or_else(|| CodexTurnFailure {
            thread_id: thread_id.clone(),
            message: "Codex workspace path is not valid UTF-8".to_string(),
            code: RunFailureCode::ConfigurationError,
            last_progress_at: None,
        })?;
        let mut params = json!({
            "cwd": workspace_text,
            "approvalPolicy": "never",
            "sandbox": "read-only",
            "config": {
                "web_search": "disabled",
                "mcp_servers": {},
                "sandbox_workspace_write": { "network_access": false }
            },
            "developerInstructions": CODEX_READ_ONLY_SYSTEM_CONTEXT
        });
        if !model.is_empty() {
            params["model"] = Value::String(model.to_string());
        }
        let method = if let Some(existing) = &thread_id {
            params["threadId"] = Value::String(existing.clone());
            "thread/resume"
        } else {
            params["ephemeral"] = Value::Bool(false);
            "thread/start"
        };
        write_codex_message(
            &mut stdin,
            json!({ "method": method, "id": 2, "params": params }),
        )
        .map_err(|message| CodexTurnFailure {
            thread_id: thread_id.clone(),
            code: RunFailureCode::ChildExited,
            message,
            last_progress_at: None,
        })?;
        let thread_response = receive_supervised_codex_result(
            &receiver,
            2,
            &mut supervisor,
            process_started,
            cancellation,
        )
        .map_err(|code| CodexTurnFailure {
            thread_id: thread_id.clone(),
            message: request_failure_message(code).to_string(),
            code,
            last_progress_at: None,
        })?;
        let resolved_thread_id = thread_response
            .pointer("/result/thread/id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| thread_id.clone())
            .ok_or_else(|| CodexTurnFailure {
                thread_id: None,
                message: "Codex thread response did not include a thread id".to_string(),
                code: RunFailureCode::ProtocolError,
                last_progress_at: None,
            })?;
        if validate_identifier(&resolved_thread_id, "Codex thread id").is_err() {
            return Err(CodexTurnFailure {
                thread_id: None,
                message: "Codex thread response included an invalid thread id".to_string(),
                code: RunFailureCode::ProtocolError,
                last_progress_at: None,
            });
        }
        thread_id = Some(resolved_thread_id.clone());
        let thread_id = resolved_thread_id;
        write_codex_message(
            &mut stdin,
            json!({
                "method": "turn/start",
                "id": 3,
                "params": {
                    "threadId": thread_id,
                    "input": [{ "type": "text", "text": prompt, "text_elements": [] }],
                    "cwd": workspace_text,
                    "approvalPolicy": "never"
                }
            }),
        )
        .map_err(|message| CodexTurnFailure {
            thread_id: Some(thread_id.clone()),
            code: RunFailureCode::ChildExited,
            message,
            last_progress_at: None,
        })?;
        let turn_response = receive_supervised_codex_result(
            &receiver,
            3,
            &mut supervisor,
            process_started,
            cancellation,
        )
        .map_err(|code| CodexTurnFailure {
            thread_id: Some(thread_id.clone()),
            message: request_failure_message(code).to_string(),
            code,
            last_progress_at: None,
        })?;
        let turn_id = turn_response
            .pointer("/result/turn/id")
            .and_then(Value::as_str)
            .ok_or_else(|| CodexTurnFailure {
                thread_id: Some(thread_id.clone()),
                message: "Codex turn response did not include a turn id".to_string(),
                code: RunFailureCode::ProtocolError,
                last_progress_at: None,
            })?
            .to_string();
        if validate_identifier(&turn_id, "Codex turn id").is_err() {
            return Err(CodexTurnFailure {
                thread_id: Some(thread_id.clone()),
                message: "Codex turn response included an invalid turn id".to_string(),
                code: RunFailureCode::ProtocolError,
                last_progress_at: None,
            });
        }
        let elapsed_ms = elapsed_millis(process_started);
        supervisor.apply(elapsed_ms, RunSignal::TurnStarted);
        let _ = on_event.send(RuntimeEvent::Started {
            run_id: run_id.to_string(),
            route: "coding.assist".to_string(),
            provider_id: "codex-sdk".to_string(),
        });
        let mut projector = CodexEventProjector::new(&thread_id, &turn_id);
        last_progress_at = Some(now_iso());
        let mut content = String::new();
        let mut content_chars = 0_usize;
        let mut failure_detail: Option<String> = None;
        let mut cancellation_observed = false;
        loop {
            let now_ms = elapsed_millis(process_started);
            let actions = if cancellation.is_cancelled() && !cancellation_observed {
                cancellation_observed = true;
                supervisor.apply(now_ms, RunSignal::CancelRequested)
            } else {
                Vec::new()
            };
            if let Some(outcome) = apply_supervisor_actions(
                &actions,
                &mut child,
                &mut stdin,
                &thread_id,
                &turn_id,
                supervisor.pending_outcome(),
            ) {
                return supervisor_outcome(
                    outcome,
                    &thread_id,
                    &content,
                    failure_detail,
                    last_progress_at.clone(),
                );
            }
            let message = match receiver.recv_timeout(supervisor_wait_duration(&supervisor, now_ms))
            {
                Ok(CodexReaderMessage::Message(message)) => message,
                Ok(CodexReaderMessage::Failed { code, message }) => {
                    failure_detail = Some(message.to_string());
                    let observed_at_ms = elapsed_millis(process_started);
                    let actions =
                        supervisor.apply(observed_at_ms, RunSignal::FailureDetected { code });
                    if let Some(outcome) = apply_supervisor_actions(
                        &actions,
                        &mut child,
                        &mut stdin,
                        &thread_id,
                        &turn_id,
                        supervisor.pending_outcome(),
                    ) {
                        return supervisor_outcome(
                            outcome,
                            &thread_id,
                            &content,
                            failure_detail,
                            last_progress_at.clone(),
                        );
                    }
                    let watch_actions = supervisor.tick(elapsed_millis(process_started));
                    if let Some(outcome) = apply_supervisor_actions(
                        &watch_actions,
                        &mut child,
                        &mut stdin,
                        &thread_id,
                        &turn_id,
                        supervisor.pending_outcome(),
                    ) {
                        return supervisor_outcome(
                            outcome,
                            &thread_id,
                            &content,
                            failure_detail,
                            last_progress_at.clone(),
                        );
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let now_ms = elapsed_millis(process_started);
                    let actions = if cancellation.is_cancelled() && !cancellation_observed {
                        cancellation_observed = true;
                        supervisor.apply(now_ms, RunSignal::CancelRequested)
                    } else {
                        supervisor.tick(now_ms)
                    };
                    if let Some(outcome) = apply_supervisor_actions(
                        &actions,
                        &mut child,
                        &mut stdin,
                        &thread_id,
                        &turn_id,
                        supervisor.pending_outcome(),
                    ) {
                        return supervisor_outcome(
                            outcome,
                            &thread_id,
                            &content,
                            failure_detail,
                            last_progress_at.clone(),
                        );
                    }
                    if child.child_mut().try_wait().ok().flatten().is_some() {
                        let actions = supervisor.apply(now_ms, RunSignal::ChildExited);
                        if let Some(outcome) = apply_supervisor_actions(
                            &actions,
                            &mut child,
                            &mut stdin,
                            &thread_id,
                            &turn_id,
                            supervisor.pending_outcome(),
                        ) {
                            return supervisor_outcome(
                                outcome,
                                &thread_id,
                                &content,
                                failure_detail,
                                last_progress_at.clone(),
                            );
                        }
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    failure_detail = Some("Codex app-server stopped during the turn".to_string());
                    let actions = supervisor.apply(now_ms, RunSignal::ChildExited);
                    let outcome = apply_supervisor_actions(
                        &actions,
                        &mut child,
                        &mut stdin,
                        &thread_id,
                        &turn_id,
                        supervisor.pending_outcome(),
                    )
                    .unwrap_or(RunOutcome::Failed(RunFailureCode::ChildExited));
                    return supervisor_outcome(
                        outcome,
                        &thread_id,
                        &content,
                        failure_detail,
                        last_progress_at.clone(),
                    );
                }
            };
            let now_ms = elapsed_millis(process_started);
            if cancellation.is_cancelled() && !cancellation_observed {
                cancellation_observed = true;
                let actions = supervisor.apply(now_ms, RunSignal::CancelRequested);
                if let Some(outcome) = apply_supervisor_actions(
                    &actions,
                    &mut child,
                    &mut stdin,
                    &thread_id,
                    &turn_id,
                    supervisor.pending_outcome(),
                ) {
                    return supervisor_outcome(
                        outcome,
                        &thread_id,
                        &content,
                        failure_detail,
                        last_progress_at.clone(),
                    );
                }
            }
            let projected = match projector.project(&message) {
                Ok(projected) => projected,
                Err(()) => {
                    failure_detail =
                        Some("Codex app-server violated the event contract".to_string());
                    ProjectedCodexEvent::ProviderError
                }
            };
            let before_progress = supervisor.last_progress_at_ms();
            let actions = match projected {
                ProjectedCodexEvent::AssistantDelta(delta) => {
                    let delta_chars = delta.chars().count();
                    let remaining = 64_000usize.saturating_sub(content_chars);
                    if delta_chars > remaining {
                        failure_detail =
                            Some("Codex response exceeded the 64,000 character limit".to_string());
                        supervisor.apply(
                            now_ms,
                            RunSignal::FailureDetected {
                                code: RunFailureCode::ResponseTooLarge,
                            },
                        )
                    } else {
                        content.push_str(&delta);
                        content_chars += delta_chars;
                        let _ = on_event.send(RuntimeEvent::Delta {
                            run_id: run_id.to_string(),
                            text: delta,
                        });
                        supervisor.apply(now_ms, RunSignal::AssistantDelta { non_empty: true })
                    }
                }
                ProjectedCodexEvent::Activity {
                    kind,
                    label,
                    summary,
                    started,
                    meaningful,
                    arms_terminal_gap,
                } => {
                    let _ = on_event.send(RuntimeEvent::Activity {
                        run_id: run_id.to_string(),
                        kind: label,
                        summary,
                    });
                    if arms_terminal_gap {
                        supervisor.apply(now_ms, RunSignal::AssistantOutputCompleted)
                    } else if meaningful {
                        supervisor.apply(
                            now_ms,
                            if started {
                                RunSignal::ItemStarted { kind }
                            } else {
                                RunSignal::ItemCompleted { kind }
                            },
                        )
                    } else {
                        Vec::new()
                    }
                }
                ProjectedCodexEvent::AssistantOutputCompleted {
                    text,
                    arms_terminal_gap,
                } => {
                    if content.is_empty() {
                        if let Some(text) = text {
                            if text.chars().count() > 64_000 {
                                failure_detail = Some(
                                    "Codex response exceeded the 64,000 character limit"
                                        .to_string(),
                                );
                                supervisor.apply(
                                    now_ms,
                                    RunSignal::FailureDetected {
                                        code: RunFailureCode::ResponseTooLarge,
                                    },
                                )
                            } else {
                                content_chars = text.chars().count();
                                content = text;
                                if arms_terminal_gap {
                                    supervisor.apply(now_ms, RunSignal::AssistantOutputCompleted)
                                } else {
                                    Vec::new()
                                }
                            }
                        } else if arms_terminal_gap {
                            supervisor.apply(now_ms, RunSignal::AssistantOutputCompleted)
                        } else {
                            Vec::new()
                        }
                    } else if arms_terminal_gap {
                        supervisor.apply(now_ms, RunSignal::AssistantOutputCompleted)
                    } else {
                        Vec::new()
                    }
                }
                ProjectedCodexEvent::Progress(kind) => {
                    supervisor.apply(now_ms, RunSignal::ItemStarted { kind })
                }
                ProjectedCodexEvent::Terminal(status) => {
                    if status != TerminalStatus::Completed {
                        failure_detail = Some(if status == TerminalStatus::Interrupted {
                            "Codex turn was interrupted".to_string()
                        } else {
                            "Codex turn failed".to_string()
                        });
                    }
                    let effective_status =
                        if status == TerminalStatus::Completed && content.trim().is_empty() {
                            failure_detail =
                                Some("Codex completed without an assistant response".to_string());
                            TerminalStatus::Failed
                        } else {
                            status
                        };
                    supervisor.apply(
                        now_ms,
                        RunSignal::Terminal {
                            status: effective_status,
                        },
                    )
                }
                ProjectedCodexEvent::PolicyViolation => {
                    failure_detail = Some(
                        "Codex attempted an operation forbidden by the read-only route".to_string(),
                    );
                    supervisor.apply(now_ms, RunSignal::PolicyViolated)
                }
                ProjectedCodexEvent::ProviderError => {
                    failure_detail.get_or_insert_with(|| "Codex turn failed".to_string());
                    supervisor.apply(
                        now_ms,
                        RunSignal::Terminal {
                            status: TerminalStatus::Failed,
                        },
                    )
                }
                ProjectedCodexEvent::Ignore => Vec::new(),
            };
            if supervisor.last_progress_at_ms() != before_progress {
                last_progress_at = Some(now_iso());
            }
            if let Some(outcome) = apply_supervisor_actions(
                &actions,
                &mut child,
                &mut stdin,
                &thread_id,
                &turn_id,
                supervisor.pending_outcome(),
            ) {
                return supervisor_outcome(
                    outcome,
                    &thread_id,
                    &content,
                    failure_detail,
                    last_progress_at.clone(),
                );
            }
            let watch_actions = supervisor.tick(elapsed_millis(process_started));
            if let Some(outcome) = apply_supervisor_actions(
                &watch_actions,
                &mut child,
                &mut stdin,
                &thread_id,
                &turn_id,
                supervisor.pending_outcome(),
            ) {
                return supervisor_outcome(
                    outcome,
                    &thread_id,
                    &content,
                    failure_detail,
                    last_progress_at.clone(),
                );
            }
        }
    })();
    drop(stdin);
    drop(receiver);
    child.terminate();
    if stdout_reader.join().is_err() && result.is_ok() {
        return Err(CodexTurnFailure {
            thread_id,
            message: "Codex output reader stopped unexpectedly".to_string(),
            code: RunFailureCode::InternalError,
            last_progress_at,
        });
    }
    result
}

fn elapsed_millis(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn supervisor_wait_duration(
    supervisor: &runtime::supervisor::RunSupervisor,
    now_ms: u64,
) -> Duration {
    let remaining_ms = supervisor
        .next_deadline_ms()
        .map(|deadline| deadline.saturating_sub(now_ms))
        .unwrap_or(100);
    Duration::from_millis(remaining_ms.clamp(1, 100))
}

fn apply_supervisor_actions(
    actions: &[runtime::contracts::SupervisorAction],
    child: &mut ProcessGuard,
    stdin: &mut impl Write,
    thread_id: &str,
    turn_id: &str,
    pending_outcome: Option<runtime::contracts::RunOutcome>,
) -> Option<runtime::contracts::RunOutcome> {
    use runtime::contracts::SupervisorAction;
    let mut outcome = None;
    for action in actions {
        match action {
            SupervisorAction::SendInterrupt => {
                if write_codex_message(
                    stdin,
                    json!({
                        "method": "turn/interrupt",
                        "id": 4,
                        "params": { "threadId": thread_id, "turnId": turn_id }
                    }),
                )
                .is_err()
                {
                    outcome = pending_outcome.or(Some(runtime::contracts::RunOutcome::Failed(
                        runtime::contracts::RunFailureCode::InternalError,
                    )));
                }
            }
            SupervisorAction::ForceKill => {
                child.terminate();
            }
            SupervisorAction::Finish(value) => outcome = Some(*value),
        }
    }
    outcome
}

fn supervisor_outcome(
    outcome: runtime::contracts::RunOutcome,
    thread_id: &str,
    content: &str,
    failure_detail: Option<String>,
    last_progress_at: Option<String>,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    use runtime::contracts::{RunFailureCode, RunOutcome};
    match outcome {
        RunOutcome::Completed => Ok(CodexTurnOutcome {
            thread_id: thread_id.to_string(),
            content: bounded_text(content, 64_000),
            last_progress_at,
        }),
        RunOutcome::Cancelled => Err(CodexTurnFailure {
            thread_id: Some(thread_id.to_string()),
            message: "Codex turn cancelled by user".to_string(),
            code: RunFailureCode::UserCancelled,
            last_progress_at,
        }),
        RunOutcome::Failed(code) => Err(CodexTurnFailure {
            thread_id: Some(thread_id.to_string()),
            message: redact_runtime_text(&failure_detail.unwrap_or_else(|| {
                match code {
                    RunFailureCode::RequestTimeout => "Codex request timed out",
                    RunFailureCode::ProgressTimeout => "Codex progress stopped",
                    RunFailureCode::TerminalTimeout => "Codex terminal event was not received",
                    RunFailureCode::HardTimeout => "Codex route reached its hard timeout",
                    RunFailureCode::ChildExited => "Codex app-server exited unexpectedly",
                    RunFailureCode::ProtocolError => "Codex app-server protocol error",
                    RunFailureCode::PolicyViolation => "Codex read-only policy violation",
                    RunFailureCode::ProviderError => "Codex turn failed",
                    RunFailureCode::ConfigurationError => "Codex configuration is invalid",
                    RunFailureCode::ChildStartFailed => "Codex app-server could not start",
                    RunFailureCode::ResponseTooLarge => "Codex response was too large",
                    RunFailureCode::InternalError => "Codex runtime internal error",
                    RunFailureCode::UserCancelled => "Codex turn cancelled by user",
                    RunFailureCode::AppRestarted => "Application restarted during the run",
                }
                .to_string()
            })),
            code,
            last_progress_at,
        }),
    }
}

fn receive_supervised_codex_result(
    receiver: &mpsc::Receiver<CodexReaderMessage>,
    request_id: u64,
    supervisor: &mut runtime::supervisor::RunSupervisor,
    origin: std::time::Instant,
    cancellation: &RunCancellation,
) -> Result<Value, runtime::contracts::RunFailureCode> {
    use runtime::contracts::{RunFailureCode, RunOutcome, RunSignal, SupervisorAction};

    supervisor.begin_request(elapsed_millis(origin));
    loop {
        if cancellation.is_cancelled() {
            supervisor.apply(elapsed_millis(origin), RunSignal::CancelRequested);
            return Err(RunFailureCode::UserCancelled);
        }
        let now_ms = elapsed_millis(origin);
        if let Some(code) = supervisor
            .tick(now_ms)
            .into_iter()
            .find_map(|action| match action {
                SupervisorAction::Finish(RunOutcome::Failed(code)) => Some(code),
                _ => None,
            })
        {
            return Err(code);
        }
        let message = match receiver.recv_timeout(supervisor_wait_duration(supervisor, now_ms)) {
            Ok(CodexReaderMessage::Message(message)) => message,
            Ok(CodexReaderMessage::Failed { code, .. }) => return Err(code),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancellation.is_cancelled() {
                    supervisor.apply(elapsed_millis(origin), RunSignal::CancelRequested);
                    return Err(RunFailureCode::UserCancelled);
                }
                let now_ms = elapsed_millis(origin);
                if let Some(code) =
                    supervisor
                        .tick(now_ms)
                        .into_iter()
                        .find_map(|action| match action {
                            SupervisorAction::Finish(RunOutcome::Failed(code)) => Some(code),
                            _ => None,
                        })
                {
                    return Err(code);
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err(RunFailureCode::ChildExited),
        };
        if message.get("id").is_some() && message.get("method").is_some() {
            return Err(RunFailureCode::PolicyViolation);
        }
        if message.get("id").and_then(Value::as_u64) != Some(request_id) {
            let now_ms = elapsed_millis(origin);
            if let Some(code) =
                supervisor
                    .tick(now_ms)
                    .into_iter()
                    .find_map(|action| match action {
                        SupervisorAction::Finish(RunOutcome::Failed(code)) => Some(code),
                        _ => None,
                    })
            {
                return Err(code);
            }
            continue;
        }
        supervisor.complete_request();
        if message.get("error").is_some() {
            return Err(RunFailureCode::ProviderError);
        }
        return Ok(message);
    }
}

fn request_failure_message(code: runtime::contracts::RunFailureCode) -> &'static str {
    use runtime::contracts::RunFailureCode;
    match code {
        RunFailureCode::UserCancelled => "Codex request was cancelled",
        RunFailureCode::RequestTimeout => "Codex request timed out",
        RunFailureCode::ChildExited => "Codex app-server stopped before responding",
        RunFailureCode::ProtocolError => "Codex app-server returned invalid output",
        RunFailureCode::PolicyViolation => "Codex requested a forbidden approval",
        RunFailureCode::ProviderError => "Codex app-server rejected the request",
        _ => "Codex request failed",
    }
}

fn load_model_providers(connection: &Connection) -> Result<ModelProvidersSettings, String> {
    let document = read_settings_document(connection, "providers.model", "default")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode provider settings: {error}"))?;
    validate_model_providers(&settings)?;
    Ok(settings)
}

fn load_routing_settings(connection: &Connection) -> Result<RoutingSettings, String> {
    let document = read_settings_document(connection, "routing.tasks", "default")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode route settings: {error}"))?;
    validate_routing_settings(&settings)?;
    Ok(settings)
}

fn load_security_settings(connection: &Connection) -> Result<SecurityRuntimeSettings, String> {
    let document = read_settings_document(connection, "security.runtime", "default")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode security settings: {error}"))?;
    validate_security_settings(&settings)?;
    Ok(settings)
}

fn effective_conversation_route_ids(
    providers: &ModelProvidersSettings,
    route: &ConversationRouteSettings,
    security: &SecurityRuntimeSettings,
) -> Vec<String> {
    let primary_is_local = providers
        .providers
        .iter()
        .find(|provider| provider.id() == route.primary_provider_id)
        .is_some_and(|provider| provider.location() == "local");
    std::iter::once(route.primary_provider_id.clone())
        .chain(route.fallback_provider_ids.iter().cloned())
        .filter(|provider_id| {
            !(security.local_only_when_selected && primary_is_local)
                || providers
                    .providers
                    .iter()
                    .find(|provider| provider.id() == *provider_id)
                    .is_none_or(|provider| provider.location() == "local")
        })
        .collect()
}

fn list_messages_from_connection(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Vec<ConversationMessage>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, conversation_id, role, content, created_at
             FROM (
               SELECT rowid AS ordinal, id, conversation_id, role, content, created_at
               FROM conversation_messages
               WHERE conversation_id = ?1
               ORDER BY CAST(created_at AS INTEGER) DESC, rowid DESC
               LIMIT 100
             )
             ORDER BY CAST(created_at AS INTEGER) ASC, ordinal ASC",
        )
        .map_err(database_error)?;
    let messages = statement
        .query_map(params![conversation_id], |row| {
            Ok(ConversationMessage {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                created_at: row.get(4)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for message in &messages {
        validate_identifier(&message.id, "message id")?;
        validate_identifier(&message.conversation_id, "conversation id")?;
        if message.conversation_id != conversation_id
            || !matches!(
                message.role.as_str(),
                "user" | "assistant" | "system" | "transcript"
            )
            || message.content.is_empty()
            || message.content.chars().count()
                > if message.role == "assistant" {
                    64_000
                } else {
                    16_000
                }
            || message.created_at.parse::<u128>().is_err()
        {
            return Err("Invalid persisted conversation message".to_string());
        }
    }
    Ok(messages)
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

fn begin_provider_session(
    state: &AppState,
    runtime_run_id: &str,
    provider_id: &str,
    provider_kind: &str,
) -> Result<String, String> {
    let session_id = new_id("provider-session");
    let now = now_iso();
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    connection
        .execute(
            "INSERT INTO provider_sessions(
               id, runtime_run_id, provider_id, provider_kind, fallback_used, output_started,
               release_status, status, started_at, updated_at
             ) VALUES (
               ?1, ?2, ?3, ?4, 0, 0,
               CASE WHEN ?4='larm' THEN 'not-started' ELSE 'not-applicable' END,
               'running', ?5, ?5
             )",
            params![session_id, runtime_run_id, provider_id, provider_kind, now],
        )
        .map_err(database_error)?;
    Ok(session_id)
}

fn persist_larm_selection(
    state: &AppState,
    session_id: &str,
    allocation: &providers::larm::contracts::ReadyAllocation,
) -> Result<(), String> {
    let selection_reason = match allocation.selection_reason {
        providers::larm::contracts::SelectionReason::Primary => "primary",
        providers::larm::contracts::SelectionReason::Other => "other",
    };
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions
             SET route_id='llm-default', allocation_id=?1, selected_runtime_id=?2,
                 fallback_used=?3, selection_reason=?4, updated_at=?5
             WHERE id=?6 AND provider_kind='larm' AND status='running' AND allocation_id IS NULL",
            params![
                allocation.allocation_id.as_str(),
                allocation.selected_runtime_id.as_str(),
                allocation.fallback_used,
                selection_reason,
                now_iso(),
                session_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("LARM provider selection could not be persisted".to_string());
    }
    Ok(())
}

fn mark_provider_output_started(state: &AppState, session_id: &str) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions SET output_started=1, updated_at=?1
             WHERE id=?2 AND status='running' AND output_started=0",
            params![now_iso(), session_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Provider output state could not be persisted".to_string());
    }
    Ok(())
}

fn mark_larm_release_pending(state: &AppState, session_id: &str) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions SET release_status='pending', updated_at=?1
             WHERE id=?2 AND provider_kind='larm' AND status='running'
               AND release_status='not-started'",
            params![now_iso(), session_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("LARM release state could not be persisted".to_string());
    }
    Ok(())
}

fn persist_larm_request_id(
    state: &AppState,
    session_id: &str,
    request_id: Option<&providers::larm::contracts::BoundedIdentifier>,
) -> Result<(), String> {
    let Some(request_id) = request_id else {
        return Ok(());
    };
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions SET request_id=?1, updated_at=?2
             WHERE id=?3 AND provider_kind='larm' AND status='running' AND request_id IS NULL",
            params![request_id.as_str(), now_iso(), session_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("LARM request correlation could not be persisted".to_string());
    }
    Ok(())
}

fn finish_larm_provider_session(
    state: &AppState,
    session_id: &str,
    status: &str,
    failure_kind: Option<ProviderFailureKind>,
    cleanup: CleanupOutcome,
) -> Result<(), String> {
    let (release_status, release_failure_kind) = match cleanup {
        CleanupOutcome::NotApplicable => ("not-applicable", None),
        CleanupOutcome::NotStarted => ("not-started", None),
        CleanupOutcome::Released => ("released", None),
        CleanupOutcome::DeferredToTtl { kind } => {
            ("deferred-to-ttl", Some(release_failure_kind_str(kind)))
        }
    };
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions
             SET status=?1, failure_reason=?2, failure_kind=?2, release_status=?3,
                 release_failure_kind=?4, updated_at=?5
             WHERE id=?6 AND provider_kind='larm' AND status='running'",
            params![
                status,
                failure_kind.map(ProviderFailureKind::as_str),
                release_status,
                release_failure_kind,
                now_iso(),
                session_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Provider session was already finalized".to_string());
    }
    Ok(())
}

fn release_failure_kind_str(kind: providers::larm::contracts::ReleaseFailureKind) -> &'static str {
    use providers::larm::contracts::ReleaseFailureKind as Release;
    match kind {
        Release::Authentication => "authentication",
        Release::Protocol => "protocol",
        Release::Upstream => "upstream",
        Release::Network => "network",
        Release::Timeout => "timeout",
        Release::Internal => "internal",
    }
}

fn finish_provider_session(
    state: &AppState,
    session_id: &str,
    status: &str,
    failure_kind: Option<ProviderFailureKind>,
) -> Result<(), String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let changed = connection
        .execute(
            "UPDATE provider_sessions
             SET status = ?1, failure_reason = ?2, failure_kind = ?2, updated_at = ?3
             WHERE id = ?4 AND status = 'running'",
            params![
                status,
                failure_kind.map(ProviderFailureKind::as_str),
                now_iso(),
                session_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Provider session was already finalized".to_string());
    }
    Ok(())
}

fn persist_conversation_success(
    state: &AppState,
    input: &StartTurnInput,
    content: &str,
) -> Result<ConversationMessage, String> {
    let content = bounded_text(content.trim(), 64_000);
    if content.is_empty() {
        return Err("Assistant message cannot be empty".to_string());
    }
    let message = ConversationMessage {
        id: new_id("message"),
        conversation_id: input.conversation_id.clone(),
        role: "assistant".to_string(),
        content,
        created_at: now_iso(),
    };
    let mut connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    let transaction = connection.transaction().map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                message.id,
                message.conversation_id,
                message.role,
                message.content,
                message.created_at
            ],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![message.created_at, input.conversation_id],
        )
        .map_err(database_error)?;
    let changed = transaction
        .execute(
            "UPDATE runtime_runs
             SET status = 'completed', error_message = NULL, completed_at = ?1
             WHERE id = ?2 AND status = 'running'",
            params![now_iso(), input.run_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Runtime run was already finalized".to_string());
    }
    transaction.commit().map_err(database_error)?;
    Ok(message)
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

fn provider_fallback_allowed(kind: ProviderFailureKind, output_started: bool) -> bool {
    !output_started
        && matches!(
            kind,
            ProviderFailureKind::Capacity
                | ProviderFailureKind::Policy
                | ProviderFailureKind::Unavailable
                | ProviderFailureKind::Draining
                | ProviderFailureKind::Upstream
                | ProviderFailureKind::Network
                | ProviderFailureKind::Timeout
                | ProviderFailureKind::AllocationLost
                | ProviderFailureKind::AllocationOutcomeUnknown
        )
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

async fn stream_model_provider(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: Arc<RunCancellation>,
    output_persistence: Option<ProviderOutputPersistence<'_>>,
) -> ProviderAttemptOutcome {
    match stream_model_provider_inner(
        provider,
        history,
        timeout_ms,
        input,
        on_event,
        cancellation,
        output_persistence,
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

struct LarmStreamContext<'a> {
    state: &'a AppState,
    session_id: &'a str,
    input: &'a StartTurnInput,
    on_event: &'a tauri::ipc::Channel<RuntimeEvent>,
}

async fn stream_larm_provider(
    provider: &LarmProviderSettings,
    history: &[ConversationMessage],
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
        let tools_enabled = context
            .state
            .connection
            .lock()
            .ok()
            .and_then(|connection| {
                memory::recall::remaining_calls(&connection, &context.input.run_id).ok()
            })
            .is_some_and(|remaining| remaining > 0)
            && tool_calls_this_attempt < memory::contracts::MAX_RECALL_CALLS_PER_TURN;
        let tools = if tools_enabled {
            vec![runtime::agent_tools::recall_tool_definition()]
        } else {
            Vec::new()
        };
        let round = larm
            .chat_with_tools(
                &mut allocation,
                &messages,
                &tool_exchanges,
                &tools,
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
                if !tools_enabled {
                    break Err(providers::larm::client::LarmError::new(
                        providers::larm::contracts::SessionFailureKind::Protocol,
                        false,
                    ));
                }
                tool_calls_this_attempt += 1;
                let content = execute_recall_tool(
                    Some(ProviderOutputPersistence {
                        state: context.state,
                        session_id: context.session_id,
                    }),
                    context.input,
                    &call,
                );
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
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: Arc<RunCancellation>,
    output_persistence: Option<ProviderOutputPersistence<'_>>,
) -> Result<String, ProviderAttemptError> {
    if cancellation.is_cancelled() {
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
        let tools_enabled = output_persistence.is_some_and(|persistence| {
            persistence
                .state
                .connection
                .lock()
                .ok()
                .and_then(|connection| {
                    memory::recall::remaining_calls(&connection, &input.run_id).ok()
                })
                .is_some_and(|remaining| remaining > 0)
                && tool_calls_this_attempt < memory::contracts::MAX_RECALL_CALLS_PER_TURN
        });
        match stream_model_provider_round(
            &client,
            provider,
            &messages,
            tools_enabled,
            input,
            on_event,
            cancellation.clone(),
            output_persistence,
            round_timeout,
        )
        .await?
        {
            ModelProviderCompletion::Content(content) => return Ok(content),
            ModelProviderCompletion::ToolCall(call) => {
                if !tools_enabled {
                    return Err(ProviderAttemptError::failed(
                        ProviderFailureKind::Protocol,
                        false,
                    ));
                }
                tool_calls_this_attempt += 1;
                let tool_content = execute_recall_tool(output_persistence, input, &call);
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
    tools_enabled: bool,
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: Arc<RunCancellation>,
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    round_timeout: Duration,
) -> Result<ModelProviderCompletion, ProviderAttemptError> {
    let mut body = json!({ "model": provider.model, "messages": messages, "stream": true });
    if tools_enabled {
        body["tools"] = json!([runtime::agent_tools::recall_tool_definition()]);
        body["tool_choice"] = json!("auto");
    }
    let mut request = client
        .post(
            provider_chat_url(&provider.endpoint)
                .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Contract, false))?,
        )
        .timeout(round_timeout)
        .json(&body);
    if let Some(api_key) = provider_api_key(provider) {
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
        .to_string();
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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum SseDrainError {
    InvalidUtf8,
    EventTooLarge,
}

fn sse_drain_failure(error: SseDrainError, output_started: bool) -> ProviderAttemptError {
    let kind = match error {
        SseDrainError::InvalidUtf8 => ProviderFailureKind::Protocol,
        SseDrainError::EventTooLarge => ProviderFailureKind::RequestTooLarge,
    };
    ProviderAttemptError::failed(kind, output_started)
}

fn drain_sse_events(
    buffer: &mut Vec<u8>,
    event_limit: usize,
) -> Result<Vec<String>, SseDrainError> {
    let mut events = Vec::new();
    loop {
        let lf = buffer.windows(2).position(|window| window == b"\n\n");
        let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
        let boundary = match (lf, crlf) {
            (Some(lf), Some(crlf)) if lf < crlf => Some((lf, 2)),
            (Some(_), Some(crlf)) => Some((crlf, 4)),
            (Some(lf), None) => Some((lf, 2)),
            (None, Some(crlf)) => Some((crlf, 4)),
            (None, None) => None,
        };
        let Some((index, delimiter_length)) = boundary else {
            break;
        };
        if index > event_limit {
            return Err(SseDrainError::EventTooLarge);
        }
        let drained = buffer.drain(..index + delimiter_length).collect::<Vec<_>>();
        events.push(
            String::from_utf8(drained[..index].to_vec()).map_err(|_| SseDrainError::InvalidUtf8)?,
        );
    }
    Ok(events)
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

async fn probe_model_provider(
    provider: &OpenAiCompatibleProviderSettings,
) -> Result<String, String> {
    let mut client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none());
    if provider.location == "local" {
        client = client.no_proxy();
    }
    let client = client
        .build()
        .map_err(|error| format!("Could not initialize HTTP client: {error}"))?;
    let mut request = client.get(provider_models_url(&provider.endpoint)?);
    if let Some(api_key) = provider_api_key(provider) {
        request = request.bearer_auth(api_key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Connection failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Provider returned HTTP {}", response.status()));
    }
    Ok("Connection succeeded".to_string())
}

fn provider_chat_url(endpoint: &str) -> Result<String, String> {
    provider_operation_url(endpoint, "chat/completions")
}

fn provider_models_url(endpoint: &str) -> Result<String, String> {
    provider_operation_url(endpoint, "models")
}

fn provider_operation_url(endpoint: &str, operation: &str) -> Result<String, String> {
    let mut url =
        url::Url::parse(endpoint).map_err(|_| "Provider endpoint is invalid".to_string())?;
    let mut path = url.path().trim_end_matches('/').to_string();
    if path.ends_with("/chat/completions") {
        path.truncate(path.len() - "/chat/completions".len());
    } else if path.ends_with("/models") {
        path.truncate(path.len() - "/models".len());
    }
    if !path.ends_with("/v1") {
        path.push_str("/v1");
    }
    path.push('/');
    path.push_str(operation);
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn provider_api_key(provider: &OpenAiCompatibleProviderSettings) -> Option<String> {
    let suffix = provider_environment_suffix(&provider.id);
    env::var(format!("SAAA_PROVIDER_{suffix}_API_KEY"))
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            (provider.location == "cloud")
                .then(|| env::var("OPENAI_API_KEY").ok())
                .flatten()
                .filter(|value| !value.is_empty())
        })
}

fn provider_environment_suffix(provider_id: &str) -> String {
    provider_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn redact_runtime_text(value: &str) -> String {
    let mut redacted = value.to_string();
    for (key, secret) in env::vars().filter(|(key, value)| {
        (key.ends_with("_API_KEY") || key.ends_with("_TOKEN")) && !value.is_empty()
    }) {
        let _ = key;
        redacted = redacted.replace(&secret, "[REDACTED]");
    }
    bounded_text(&redacted, 2_000)
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

fn save_settings_documents_to_connection(
    connection: &mut Connection,
    documents: &[SaveSettingsDocumentInput],
) -> Result<Vec<SettingsDocument>, String> {
    if documents.is_empty() {
        return Err("No settings documents to save".to_string());
    }
    for document in documents {
        validate_settings_document(document)?;
    }
    validate_settings_batch(documents)?;
    let transaction = connection.transaction().map_err(database_error)?;
    for document in documents {
        let value_text = serde_json::to_string(&document.value_json)
            .map_err(|error| format!("Could not encode settings: {error}"))?;
        transaction
            .execute(
                "INSERT INTO settings_documents(namespace, key, schema_version, value_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(namespace, key) DO UPDATE SET
                   schema_version = excluded.schema_version,
                   value_json = excluded.value_json,
                   updated_at = excluded.updated_at",
                params![
                    document.namespace,
                    document.key,
                    document.schema_version,
                    value_text,
                    now_iso()
                ],
            )
            .map_err(database_error)?;
    }
    transaction.commit().map_err(database_error)?;

    let saved = documents
        .iter()
        .map(|document| read_settings_document(connection, &document.namespace, &document.key))
        .collect::<Result<Vec<_>, _>>()?;
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

fn ensure_primary_conversation(connection: &Connection) -> Result<Conversation, String> {
    let now = now_iso();
    connection
        .execute(
            "INSERT OR IGNORE INTO conversations(id, title, task_mode, created_at, updated_at)
             VALUES (?1, ?2, 'conversation', ?3, ?3)",
            params![PRIMARY_CONVERSATION_ID, PRIMARY_CONVERSATION_TITLE, now],
        )
        .map_err(database_error)?;
    let conversation = connection
        .query_row(
            "SELECT id, title, task_mode, created_at, updated_at
             FROM conversations WHERE id = ?1",
            params![PRIMARY_CONVERSATION_ID],
            |row| {
                Ok(Conversation {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    task_mode: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                })
            },
        )
        .map_err(database_error)?;
    if conversation.task_mode != "conversation" {
        return Err("Primary conversation has an invalid task mode".to_string());
    }
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
    let exists: bool = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
            params![message.conversation_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !exists {
        return Err("Conversation does not exist".to_string());
    }
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

fn initialize_database(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         CREATE TABLE IF NOT EXISTS settings_documents (
           namespace TEXT NOT NULL,
           key TEXT NOT NULL,
           schema_version INTEGER NOT NULL,
           value_json TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           PRIMARY KEY(namespace, key)
         );
         CREATE TABLE IF NOT EXISTS conversations (
           id TEXT PRIMARY KEY,
           title TEXT,
           task_mode TEXT NOT NULL CHECK(task_mode IN ('conversation', 'coding')),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS conversation_messages (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL,
           role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system', 'transcript')),
           content TEXT NOT NULL,
           created_at TEXT NOT NULL,
           FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_conversation_messages_conversation_created
           ON conversation_messages(conversation_id, created_at);
         CREATE TABLE IF NOT EXISTS provider_sessions (
           id TEXT PRIMARY KEY,
           provider_id TEXT NOT NULL,
           runtime_run_id TEXT CHECK(runtime_run_id IS NULL OR (length(runtime_run_id) BETWEEN 1 AND 160 AND runtime_run_id NOT GLOB '*[^A-Za-z0-9_-]*')),
           provider_kind TEXT CHECK(provider_kind IS NULL OR provider_kind IN ('openai-compatible', 'larm')),
           route_id TEXT CHECK(route_id IS NULL OR (length(route_id) BETWEEN 1 AND 80 AND route_id NOT GLOB '*[^A-Za-z0-9._-]*')),
           allocation_id TEXT CHECK(allocation_id IS NULL OR (length(allocation_id) BETWEEN 1 AND 160 AND allocation_id NOT GLOB '*[^A-Za-z0-9_-]*')),
           selected_runtime_id TEXT CHECK(selected_runtime_id IS NULL OR (length(selected_runtime_id) BETWEEN 1 AND 160 AND selected_runtime_id NOT GLOB '*[^A-Za-z0-9_-]*')),
           fallback_used INTEGER CHECK(fallback_used IS NULL OR fallback_used IN (0,1)),
           selection_reason TEXT CHECK(selection_reason IS NULL OR selection_reason IN ('primary', 'other')),
           request_id TEXT CHECK(request_id IS NULL OR (length(request_id) BETWEEN 1 AND 160 AND request_id NOT GLOB '*[^A-Za-z0-9_-]*')),
           output_started INTEGER CHECK(output_started IS NULL OR output_started IN (0,1)),
           failure_kind TEXT CHECK(failure_kind IS NULL OR failure_kind IN (
             'authentication','contract','protocol','request-too-large','internal','client-disconnected',
             'cancelled','partial-output','policy','capacity','unavailable','draining','upstream','network',
             'timeout','allocation-lost','allocation-outcome-unknown','not-ready'
           )),
           release_status TEXT NOT NULL DEFAULT 'not-applicable' CHECK(release_status IN ('not-applicable','not-started','pending','released','failed','deferred-to-ttl')),
           release_failure_kind TEXT CHECK(release_failure_kind IS NULL OR release_failure_kind IN ('network','timeout','authentication','protocol','upstream','internal')),
           status TEXT NOT NULL CHECK(status IN ('running', 'completed', 'failed', 'cancelled', 'interrupted')),
           failure_reason TEXT,
           started_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS runtime_runs (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL,
           route_kind TEXT NOT NULL CHECK(route_kind IN ('conversation.respond', 'coding.assist', 'voice.transcribe', 'voice.speak')),
           provider_id TEXT,
           status TEXT NOT NULL CHECK(status IN ('running', 'completed', 'failed', 'cancelled', 'interrupted')),
           error_message TEXT,
           failure_code TEXT CHECK(failure_code IS NULL OR failure_code IN (
             'user-cancelled','app-restarted','configuration-error','child-start-failed',
             'request-timeout','progress-timeout',
             'terminal-timeout','hard-timeout','child-exited','protocol-error',
             'policy-violation','provider-error','response-too-large','internal-error'
           )),
           supervisor_version TEXT CHECK(supervisor_version IS NULL OR length(supervisor_version) BETWEEN 1 AND 64),
           last_progress_at TEXT CHECK(last_progress_at IS NULL OR length(last_progress_at) BETWEEN 1 AND 32),
           input_message_id TEXT,
           started_at TEXT NOT NULL,
           completed_at TEXT,
           FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_runtime_runs_conversation_started
           ON runtime_runs(conversation_id, started_at);
         CREATE TABLE IF NOT EXISTS codex_threads (
           conversation_id TEXT PRIMARY KEY,
           thread_id TEXT NOT NULL UNIQUE,
           model TEXT NOT NULL,
           workspace_path TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS situation_ledger (
           id TEXT PRIMARY KEY,
           observed_at TEXT NOT NULL,
           scene TEXT NOT NULL,
           confidence INTEGER NOT NULL CHECK(confidence BETWEEN 0 AND 100),
           user_attention TEXT NOT NULL CHECK(user_attention IN ('available', 'busy', 'unknown')),
           audio_environment TEXT NOT NULL CHECK(audio_environment IN ('silence', 'speech', 'multi-speaker', 'media', 'unknown')),
           proposed_attention TEXT NOT NULL CHECK(proposed_attention IN ('IGNORE', 'OBSERVE', 'SUGGEST', 'RESPOND')),
           actual_execution TEXT NOT NULL CHECK(actual_execution = 'NONE'),
           actual_presentation TEXT NOT NULL CHECK(actual_presentation = 'SILENT'),
           evidence_json TEXT NOT NULL,
           signal_health_json TEXT NOT NULL,
           decision_reasons_json TEXT NOT NULL,
           rule_version TEXT NOT NULL,
           policy_version TEXT NOT NULL,
           entry_kind TEXT NOT NULL CHECK(entry_kind IN ('transition', 'decision', 'heartbeat'))
         );
         CREATE INDEX IF NOT EXISTS idx_situation_ledger_observed
           ON situation_ledger(observed_at DESC);
         CREATE TABLE IF NOT EXISTS situation_feedback (
           ledger_id TEXT PRIMARY KEY,
           verdict TEXT NOT NULL CHECK(verdict IN ('accurate', 'inaccurate', 'unsure')),
           corrected_scene TEXT,
           created_at TEXT NOT NULL,
           FOREIGN KEY(ledger_id) REFERENCES situation_ledger(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS meeting_sessions (
           id TEXT PRIMARY KEY,
           status TEXT NOT NULL CHECK(status IN ('active','paused','completed','saved','discarded','failed','interrupted')),
           microphone_enabled INTEGER NOT NULL CHECK(microphone_enabled IN (0,1)),
           system_audio_enabled INTEGER NOT NULL CHECK(system_audio_enabled IN (0,1)),
           stt_provider_id TEXT NOT NULL CHECK(stt_provider_id IN ('local-whisper','gnosis-asr')),
           stt_model_label TEXT NOT NULL CHECK(length(stt_model_label) <= 256),
           translation_provider_id TEXT,
           persistence_mode TEXT NOT NULL CHECK(persistence_mode IN ('discard','explicit-save')),
           started_at TEXT NOT NULL, ended_at TEXT, saved_at TEXT, error_code TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_meeting_sessions_started ON meeting_sessions(started_at DESC);
         CREATE TABLE IF NOT EXISTS meeting_transcript_entries (
           id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
           lane TEXT NOT NULL CHECK(lane IN ('microphone','system-audio')),
           sequence INTEGER NOT NULL CHECK(sequence >= 0),
           original_text TEXT NOT NULL CHECK(length(original_text) BETWEEN 1 AND 8000),
           original_language TEXT, translated_text TEXT CHECK(translated_text IS NULL OR length(translated_text) <= 8000), translated_language TEXT,
           started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0), ended_at_ms INTEGER NOT NULL CHECK(ended_at_ms >= started_at_ms), created_at TEXT NOT NULL,
           FOREIGN KEY(session_id) REFERENCES meeting_sessions(id) ON DELETE CASCADE, UNIQUE(session_id,lane,sequence)
         );
         CREATE INDEX IF NOT EXISTS idx_meeting_transcript_session_sequence ON meeting_transcript_entries(session_id,lane,sequence);",
    )?;

    let transaction = connection.unchecked_transaction()?;
    migrate_legacy_settings_documents(&transaction)?;
    migrate_v4_to_v5(&transaction)?;
    migrate_v6_to_v7(&transaction)?;
    migrate_v7_to_v8(&transaction)?;
    migrate_v8_to_v9(&transaction)?;
    memory::recall::migrate_v9_to_v10(&transaction)?;
    voice::profile::migrate_v10_to_v11(&transaction)?;
    transaction.execute("UPDATE settings_documents SET schema_version = 9, updated_at = ?1 WHERE schema_version < 9", params![now_iso()])?;

    for (namespace, key, schema_version, value) in default_settings_documents() {
        transaction.execute(
            "INSERT OR IGNORE INTO settings_documents(namespace, key, schema_version, value_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![namespace, key, schema_version, value.to_string(), now_iso()],
        )?;
    }
    migrate_pristine_provider_defaults_to_gnosis(&transaction)?;
    reconcile_interrupted_runs(&transaction)?;
    meeting::reconcile(&transaction)?;
    transaction.pragma_update(None, "user_version", 11)?;
    transaction.commit()
}

fn migrate_v4_to_v5(connection: &Connection) -> rusqlite::Result<()> {
    let has_impact: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('situation_feedback') WHERE name='impact')",
        [],
        |r| r.get(0),
    )?;
    if !has_impact {
        connection.execute_batch("ALTER TABLE situation_feedback ADD COLUMN impact TEXT NOT NULL DEFAULT 'none' CHECK(impact IN ('none','no-effect','harmful'));")?;
    }
    let has_reason: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('situation_feedback') WHERE name='reason_code')",
        [],
        |row| row.get(0),
    )?;
    if !has_reason {
        connection.execute_batch("ALTER TABLE situation_feedback ADD COLUMN reason_code TEXT CHECK(reason_code IS NULL OR reason_code IN ('wrong-scene','stale-signal','unstable-transition','unwanted-suggestion','missed-meeting-candidate','insufficient-evidence'));")?;
    }
    connection.execute_batch("CREATE TABLE IF NOT EXISTS situation_quality_windows (id TEXT PRIMARY KEY, started_at TEXT NOT NULL, ended_at TEXT NOT NULL, rule_version TEXT NOT NULL, counters_json TEXT NOT NULL CHECK(length(counters_json)<=4096), created_at TEXT NOT NULL); CREATE INDEX IF NOT EXISTS idx_situation_quality_windows_ended ON situation_quality_windows(CAST(ended_at AS INTEGER) DESC); CREATE TABLE IF NOT EXISTS situation_calibration_profiles (id TEXT PRIMARY KEY, rule_version TEXT NOT NULL UNIQUE, base_rule_version TEXT, status TEXT NOT NULL CHECK(status IN ('candidate','active','superseded','rejected','rolled-back')), parameters_json TEXT NOT NULL CHECK(length(parameters_json)<=2048), created_at TEXT NOT NULL, decided_at TEXT, decision_reason_code TEXT CHECK(decision_reason_code IS NULL OR decision_reason_code IN ('wrong-scene','stale-signal','unstable-transition','unwanted-suggestion','missed-meeting-candidate','insufficient-evidence')), FOREIGN KEY(base_rule_version) REFERENCES situation_calibration_profiles(rule_version)); CREATE UNIQUE INDEX IF NOT EXISTS idx_situation_calibration_one_active ON situation_calibration_profiles(status) WHERE status='active'; CREATE TABLE IF NOT EXISTS situation_calibration_runs (id TEXT PRIMARY KEY, profile_id TEXT NOT NULL, fixture_set_version TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('completed','failed')), metrics_json TEXT CHECK(metrics_json IS NULL OR length(metrics_json)<=8192), error_code TEXT, started_at TEXT NOT NULL, completed_at TEXT NOT NULL, FOREIGN KEY(profile_id) REFERENCES situation_calibration_profiles(id) ON DELETE CASCADE); CREATE INDEX IF NOT EXISTS idx_situation_calibration_runs_completed ON situation_calibration_runs(completed_at DESC);")?;
    let parameters = serde_json::to_string(&situation::contracts::CalibrationParameters::default())
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    connection.execute("INSERT OR IGNORE INTO situation_calibration_profiles(id,rule_version,status,parameters_json,created_at,decided_at) VALUES('profile_mvp1_default','mvp1-rules-v1','active',?1,?2,?2)", params![parameters, now_iso()])?;
    Ok(())
}

fn migrate_v6_to_v7(connection: &Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 7 {
        return Ok(());
    }
    for (column, definition) in [
        (
            "failure_code",
            "TEXT CHECK(failure_code IS NULL OR failure_code IN ('user-cancelled','app-restarted','configuration-error','child-start-failed','request-timeout','progress-timeout','terminal-timeout','hard-timeout','child-exited','protocol-error','policy-violation','provider-error','response-too-large','internal-error'))",
        ),
        (
            "supervisor_version",
            "TEXT CHECK(supervisor_version IS NULL OR length(supervisor_version) BETWEEN 1 AND 64)",
        ),
        (
            "last_progress_at",
            "TEXT CHECK(last_progress_at IS NULL OR length(last_progress_at) BETWEEN 1 AND 32)",
        ),
    ] {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('runtime_runs') WHERE name=?1)",
            [column],
            |row| row.get(0),
        )?;
        if !exists {
            connection.execute_batch(&format!(
                "ALTER TABLE runtime_runs ADD COLUMN {column} {definition};"
            ))?;
        }
    }
    for (namespace, key, _, template) in default_settings_documents() {
        let template = settings_template_for_v7(namespace, template);
        let legacy: Option<String> = connection
            .query_row(
                "SELECT value_json FROM settings_documents
                 WHERE namespace=?1 AND key=?2 AND schema_version < 7",
                params![namespace, key],
                |row| row.get(0),
            )
            .optional()?;
        let Some(legacy) = legacy else {
            continue;
        };
        let value: Value = serde_json::from_str(&legacy).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let normalized = normalize_json_to_template(&value, &template);
        connection.execute(
            "UPDATE settings_documents
             SET schema_version=7, value_json=?1, updated_at=?2
             WHERE namespace=?3 AND key=?4",
            params![normalized.to_string(), now_iso(), namespace, key],
        )?;
    }
    Ok(())
}

fn settings_template_for_v7(namespace: &str, mut template: Value) -> Value {
    if namespace == "providers.model" {
        if let Some(providers) = template.get_mut("providers").and_then(Value::as_array_mut) {
            for provider in providers {
                if let Some(provider) = provider.as_object_mut() {
                    provider.remove("kind");
                }
            }
        }
    }
    template
}

fn migrate_v7_to_v8(connection: &Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 8 {
        return Ok(());
    }
    for (column, definition) in [
        (
            "runtime_run_id",
            "TEXT CHECK(runtime_run_id IS NULL OR (length(runtime_run_id) BETWEEN 1 AND 160 AND runtime_run_id NOT GLOB '*[^A-Za-z0-9_-]*'))",
        ),
        (
            "provider_kind",
            "TEXT CHECK(provider_kind IS NULL OR provider_kind IN ('openai-compatible', 'larm'))",
        ),
        (
            "route_id",
            "TEXT CHECK(route_id IS NULL OR (length(route_id) BETWEEN 1 AND 80 AND route_id NOT GLOB '*[^A-Za-z0-9._-]*'))",
        ),
        (
            "allocation_id",
            "TEXT CHECK(allocation_id IS NULL OR (length(allocation_id) BETWEEN 1 AND 160 AND allocation_id NOT GLOB '*[^A-Za-z0-9_-]*'))",
        ),
        (
            "selected_runtime_id",
            "TEXT CHECK(selected_runtime_id IS NULL OR (length(selected_runtime_id) BETWEEN 1 AND 160 AND selected_runtime_id NOT GLOB '*[^A-Za-z0-9_-]*'))",
        ),
        (
            "fallback_used",
            "INTEGER CHECK(fallback_used IS NULL OR fallback_used IN (0,1))",
        ),
        (
            "selection_reason",
            "TEXT CHECK(selection_reason IS NULL OR selection_reason IN ('primary', 'other'))",
        ),
        (
            "request_id",
            "TEXT CHECK(request_id IS NULL OR (length(request_id) BETWEEN 1 AND 160 AND request_id NOT GLOB '*[^A-Za-z0-9_-]*'))",
        ),
        (
            "output_started",
            "INTEGER CHECK(output_started IS NULL OR output_started IN (0,1))",
        ),
        (
            "failure_kind",
            "TEXT CHECK(failure_kind IS NULL OR failure_kind IN ('authentication','contract','protocol','request-too-large','internal','client-disconnected','cancelled','partial-output','policy','capacity','unavailable','draining','upstream','network','timeout','allocation-lost','allocation-outcome-unknown','not-ready'))",
        ),
        (
            "release_status",
            "TEXT NOT NULL DEFAULT 'not-applicable' CHECK(release_status IN ('not-applicable','not-started','pending','released','failed','deferred-to-ttl'))",
        ),
        (
            "release_failure_kind",
            "TEXT CHECK(release_failure_kind IS NULL OR release_failure_kind IN ('network','timeout','authentication','protocol','upstream','internal'))",
        ),
    ] {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('provider_sessions') WHERE name=?1)",
            [column],
            |row| row.get(0),
        )?;
        if !exists {
            connection.execute_batch(&format!(
                "ALTER TABLE provider_sessions ADD COLUMN {column} {definition};"
            ))?;
        }
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_provider_sessions_runtime_run
         ON provider_sessions(runtime_run_id);",
    )?;

    for (namespace, key, _, template) in default_settings_documents() {
        let legacy: Option<String> = connection
            .query_row(
                "SELECT value_json FROM settings_documents
                 WHERE namespace=?1 AND key=?2 AND schema_version < 8",
                params![namespace, key],
                |row| row.get(0),
            )
            .optional()?;
        let Some(legacy) = legacy else {
            continue;
        };
        let value: Value = serde_json::from_str(&legacy).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let normalized = normalize_json_to_template(&value, &template);
        connection.execute(
            "UPDATE settings_documents
             SET schema_version=8, value_json=?1, updated_at=?2
             WHERE namespace=?3 AND key=?4",
            params![normalized.to_string(), now_iso(), namespace, key],
        )?;
    }
    Ok(())
}

fn migrate_v8_to_v9(connection: &Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 9 {
        return Ok(());
    }

    let meeting_schema: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='meeting_sessions'",
        [],
        |row| row.get(0),
    )?;
    if !meeting_schema.contains("gnosis-asr") {
        connection.execute_batch(
            "CREATE TABLE meeting_sessions_v9 (
               id TEXT PRIMARY KEY,
               status TEXT NOT NULL CHECK(status IN ('active','paused','completed','saved','discarded','failed','interrupted')),
               microphone_enabled INTEGER NOT NULL CHECK(microphone_enabled IN (0,1)),
               system_audio_enabled INTEGER NOT NULL CHECK(system_audio_enabled IN (0,1)),
               stt_provider_id TEXT NOT NULL CHECK(stt_provider_id IN ('local-whisper','gnosis-asr')),
               stt_model_label TEXT NOT NULL CHECK(length(stt_model_label) <= 256),
               translation_provider_id TEXT,
               persistence_mode TEXT NOT NULL CHECK(persistence_mode IN ('discard','explicit-save')),
               started_at TEXT NOT NULL, ended_at TEXT, saved_at TEXT, error_code TEXT
             );
             INSERT INTO meeting_sessions_v9
               SELECT * FROM meeting_sessions;
             CREATE TABLE meeting_transcript_entries_v9 (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               lane TEXT NOT NULL CHECK(lane IN ('microphone','system-audio')),
               sequence INTEGER NOT NULL CHECK(sequence >= 0),
               original_text TEXT NOT NULL CHECK(length(original_text) BETWEEN 1 AND 8000),
               original_language TEXT,
               translated_text TEXT CHECK(translated_text IS NULL OR length(translated_text) <= 8000),
               translated_language TEXT,
               started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0),
               ended_at_ms INTEGER NOT NULL CHECK(ended_at_ms >= started_at_ms),
               created_at TEXT NOT NULL,
               FOREIGN KEY(session_id) REFERENCES meeting_sessions_v9(id) ON DELETE CASCADE,
               UNIQUE(session_id,lane,sequence)
             );
             INSERT INTO meeting_transcript_entries_v9
               SELECT * FROM meeting_transcript_entries;
             DROP TABLE meeting_transcript_entries;
             DROP TABLE meeting_sessions;
             ALTER TABLE meeting_sessions_v9 RENAME TO meeting_sessions;
             ALTER TABLE meeting_transcript_entries_v9 RENAME TO meeting_transcript_entries;
             CREATE INDEX idx_meeting_sessions_started ON meeting_sessions(started_at DESC);
             CREATE INDEX idx_meeting_transcript_session_sequence
               ON meeting_transcript_entries(session_id,lane,sequence);",
        )?;
    }

    for (namespace, key, _, template) in default_settings_documents() {
        let legacy: Option<String> = connection
            .query_row(
                "SELECT value_json FROM settings_documents
                 WHERE namespace=?1 AND key=?2 AND schema_version < 9",
                params![namespace, key],
                |row| row.get(0),
            )
            .optional()?;
        let Some(legacy) = legacy else {
            continue;
        };
        let value: Value = serde_json::from_str(&legacy).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let mut normalized = normalize_json_to_template(&value, &template);
        if namespace == "voice.runtime" {
            normalized["sttProviderId"] = json!(voice::gnosis_asr::PROVIDER_ID);
            normalized["sttModel"] = json!(voice::gnosis_asr::MODEL_ID);
        }
        connection.execute(
            "UPDATE settings_documents
             SET schema_version=9, value_json=?1, updated_at=?2
             WHERE namespace=?3 AND key=?4",
            params![normalized.to_string(), now_iso(), namespace, key],
        )?;
    }
    Ok(())
}

fn normalize_json_to_template(value: &Value, template: &Value) -> Value {
    match (value, template) {
        (Value::Object(value), Value::Object(template)) => Value::Object(
            template
                .iter()
                .map(|(key, template_value)| {
                    let normalized = value
                        .get(key)
                        .map(|value| normalize_json_to_template(value, template_value))
                        .unwrap_or_else(|| template_value.clone());
                    (key.clone(), normalized)
                })
                .collect(),
        ),
        (Value::Array(value), Value::Array(template)) if template.len() == 1 => Value::Array(
            value
                .iter()
                .map(|value| normalize_json_to_template(value, &template[0]))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn backup_connection_to(source: &Connection, path: &std::path::Path) -> Result<(), String> {
    let mut destination = Connection::open(path)
        .map_err(|error| format!("Could not create the database backup: {error}"))?;
    let backup = rusqlite::backup::Backup::new(source, &mut destination)
        .map_err(|error| format!("Could not initialize the database backup: {error}"))?;
    backup
        .run_to_completion(32, Duration::from_millis(20), None)
        .map_err(|error| format!("Database backup failed: {error}"))
}

fn backup_before_migration(
    connection: &Connection,
    database_path: &std::path::Path,
) -> Result<Option<PathBuf>, String> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(database_error)?;
    let has_data = fs::metadata(database_path)
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false);
    if !has_data || version >= 11 {
        return Ok(None);
    }
    let directory = database_path
        .parent()
        .ok_or_else(|| "Database path has no parent directory".to_string())?
        .join("backups");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the migration backup directory: {error}"))?;
    let path = directory.join(format!("pre-migration-{}.sqlite3", now_iso()));
    backup_connection_to(connection, &path)?;
    Ok(Some(path))
}

fn default_settings_documents() -> Vec<(&'static str, &'static str, i64, Value)> {
    vec![
        (
            "providers.model",
            "default",
            9,
            json!({
                "providers": [{
                    "kind": "openai-compatible",
                    "id": GNOSIS_PROVIDER_ID,
                    "enabled": true,
                    "label": "gnosis · Qwen3.8 27B",
                    "location": "local",
                    "endpoint": GNOSIS_ENDPOINT,
                    "model": GNOSIS_MODEL,
                    "credentialStatus": "not-configured"
                }]
            }),
        ),
        (
            "providers.agent",
            "codex-sdk",
            9,
            json!({
                "enabled": false,
                "provider": "codex-sdk",
                "model": "",
                "runtimeMode": "app-server",
                "health": "unchecked",
                "sandboxMode": "read-only",
                "approvalPolicy": "never",
                "networkEnabled": false,
                "webSearchEnabled": false,
                "workspacePolicy": "select-per-conversation"
            }),
        ),
        (
            "routing.tasks",
            "default",
            9,
            json!({
                "conversationRespond": {
                    "primaryProviderId": GNOSIS_PROVIDER_ID,
                    "fallbackProviderIds": [],
                    "timeoutMs": 30000
                },
                "codingAssist": {
                    "providerId": "codex-sdk",
                    "timeoutMs": 120000,
                    "readOnly": true,
                    "networkEnabled": false,
                    "webSearchEnabled": false
                }
            }),
        ),
        (
            "voice.runtime",
            "default",
            9,
            json!({
                "inputDeviceId": "default",
                "outputDeviceId": "default",
                "captureMode": "push-to-talk",
                    "sttProviderId": "gnosis-asr",
                "sttModel": "qwen3-asr-1.7b",
                    "ttsProviderId": "system-tts",
                "ttsVoice": "default",
                "autoSpeak": true,
                "cloudFallbackEnabled": false
            }),
        ),
        (
            "security.runtime",
            "default",
            9,
            json!({
                "credentialStorage": "environment",
                "localOnlyWhenSelected": true,
                "diagnosticsRedaction": true
            }),
        ),
        (
            "situation.runtime",
            "default",
            9,
            serde_json::to_value(situation::contracts::SituationRuntimeSettings::default())
                .expect("default Situation settings serialize"),
        ),
    ]
}

fn migrate_pristine_provider_defaults_to_gnosis(connection: &Connection) -> rusqlite::Result<()> {
    let legacy_providers = json!({
        "providers": [{
            "kind": "openai-compatible",
            "id": "local-openai-compatible",
            "enabled": false,
            "label": "Local OpenAI-compatible",
            "location": "local",
            "endpoint": "",
            "model": "",
            "credentialStatus": "not-configured"
        }]
    });
    let current: Option<(String, String)> = connection
        .query_row(
            "SELECT providers.value_json, routing.value_json
             FROM settings_documents AS providers
             JOIN settings_documents AS routing
               ON routing.namespace='routing.tasks' AND routing.key='default'
             WHERE providers.namespace='providers.model' AND providers.key='default'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((providers_text, routing_text)) = current else {
        return Ok(());
    };
    let providers: Value = serde_json::from_str(&providers_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let routing: Value = serde_json::from_str(&routing_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    if providers != legacy_providers
        || routing.pointer("/conversationRespond/primaryProviderId")
            != Some(&json!("local-openai-compatible"))
    {
        return Ok(());
    }

    let defaults = default_settings_documents();
    let gnosis_providers = defaults
        .iter()
        .find(|(namespace, key, _, _)| *namespace == "providers.model" && *key == "default")
        .map(|(_, _, _, value)| value)
        .expect("providers default exists");
    let mut gnosis_routing = routing;
    gnosis_routing["conversationRespond"]["primaryProviderId"] = json!(GNOSIS_PROVIDER_ID);
    let updated_at = now_iso();
    connection.execute(
        "UPDATE settings_documents SET value_json=?1, updated_at=?2
         WHERE namespace='providers.model' AND key='default'",
        params![gnosis_providers.to_string(), updated_at],
    )?;
    connection.execute(
        "UPDATE settings_documents SET value_json=?1, updated_at=?2
         WHERE namespace='routing.tasks' AND key='default'",
        params![gnosis_routing.to_string(), updated_at],
    )?;
    Ok(())
}

fn migrate_legacy_settings_documents(connection: &Connection) -> rusqlite::Result<()> {
    migrate_document(connection, "providers.model", "default", |legacy| {
        let provider = json!({
            "id": legacy.get("id").and_then(Value::as_str).unwrap_or("local-openai-compatible"),
            "enabled": legacy.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            "label": legacy.get("label").and_then(Value::as_str).unwrap_or("Local OpenAI-compatible"),
            "location": legacy.get("location").and_then(Value::as_str).unwrap_or("local"),
            "endpoint": legacy.get("endpoint").and_then(Value::as_str).unwrap_or(""),
            "model": legacy.get("model").and_then(Value::as_str).unwrap_or(""),
            "credentialStatus": legacy.get("credentialStatus").and_then(Value::as_str).unwrap_or("not-configured")
        });
        json!({ "providers": [provider] })
    })?;
    migrate_document(connection, "providers.agent", "codex-sdk", |legacy| {
        json!({
            "enabled": legacy.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            "provider": "codex-sdk",
            "model": legacy.get("model").and_then(Value::as_str).unwrap_or(""),
            "runtimeMode": "app-server",
            "health": legacy.get("health").and_then(Value::as_str).unwrap_or("unchecked"),
            "sandboxMode": "read-only",
            "approvalPolicy": "never",
            "networkEnabled": false,
            "webSearchEnabled": false,
            "workspacePolicy": "select-per-conversation"
        })
    })?;
    migrate_document(connection, "routing.tasks", "default", |legacy| {
        json!({
            "conversationRespond": legacy.get("conversationRespond").cloned().unwrap_or_else(|| json!({
                "primaryProviderId": "local-openai-compatible", "fallbackProviderIds": [], "timeoutMs": 30000
            })),
            "codingAssist": {
                "providerId": "codex-sdk",
                "timeoutMs": legacy.pointer("/codingAssist/timeoutMs").and_then(Value::as_u64).unwrap_or(120000),
                "readOnly": true,
                "networkEnabled": false,
                "webSearchEnabled": false
            }
        })
    })?;
    migrate_document(connection, "voice.runtime", "default", |legacy| {
        json!({
            "inputDeviceId": legacy.get("inputDeviceId").and_then(Value::as_str).unwrap_or("default"),
            "outputDeviceId": legacy.get("outputDeviceId").and_then(Value::as_str).unwrap_or("default"),
            "captureMode": "push-to-talk",
            "sttProviderId": "gnosis-asr",
            "sttModel": "qwen3-asr-1.7b",
            "ttsProviderId": "system-tts",
            "ttsVoice": legacy.get("ttsVoice").and_then(Value::as_str).unwrap_or("default"),
            "autoSpeak": legacy.get("autoSpeak").and_then(Value::as_bool).unwrap_or(true),
            "cloudFallbackEnabled": false
        })
    })?;
    migrate_document(connection, "security.runtime", "default", |legacy| {
        json!({
            "credentialStorage": "environment",
            "localOnlyWhenSelected": legacy.get("localOnlyWhenSelected").and_then(Value::as_bool).unwrap_or(true),
            "diagnosticsRedaction": true
        })
    })?;
    Ok(())
}

fn migrate_document(
    connection: &Connection,
    namespace: &str,
    key: &str,
    transform: impl FnOnce(Value) -> Value,
) -> rusqlite::Result<()> {
    let legacy: Option<(i64, String)> = connection
        .query_row(
            "SELECT schema_version, value_json FROM settings_documents WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((schema_version, value_text)) = legacy else {
        return Ok(());
    };
    if schema_version >= 3 {
        return Ok(());
    }
    let legacy_value = serde_json::from_str(&value_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    connection.execute(
        "UPDATE settings_documents SET schema_version = 3, value_json = ?1, updated_at = ?2
         WHERE namespace = ?3 AND key = ?4",
        params![
            transform(legacy_value).to_string(),
            now_iso(),
            namespace,
            key
        ],
    )?;
    Ok(())
}

fn validate_settings_document(input: &SaveSettingsDocumentInput) -> Result<(), String> {
    let allowed = matches!(
        (input.namespace.as_str(), input.key.as_str()),
        ("providers.model", "default")
            | ("providers.agent", "codex-sdk")
            | ("routing.tasks", "default")
            | ("voice.runtime", "default")
            | ("security.runtime", "default")
            | ("situation.runtime", "default")
    );
    if !allowed {
        return Err("Unsupported settings document".to_string());
    }
    if input.schema_version != 9 || !input.value_json.is_object() {
        return Err("Invalid settings schema".to_string());
    }
    match (input.namespace.as_str(), input.key.as_str()) {
        ("providers.model", "default") => {
            let settings =
                serde_json::from_value::<ModelProvidersSettings>(input.value_json.clone())
                    .map_err(|error| format!("Invalid model provider settings: {error}"))?;
            validate_model_providers(&settings)
        }
        ("providers.agent", "codex-sdk") => {
            let settings =
                serde_json::from_value::<CodexAgentRuntimeSettings>(input.value_json.clone())
                    .map_err(|error| format!("Invalid Codex settings: {error}"))?;
            validate_codex_settings(&settings)
        }
        ("routing.tasks", "default") => {
            let settings = serde_json::from_value::<RoutingSettings>(input.value_json.clone())
                .map_err(|error| format!("Invalid routing settings: {error}"))?;
            validate_routing_settings(&settings)
        }
        ("voice.runtime", "default") => {
            let settings = serde_json::from_value::<VoiceRuntimeSettings>(input.value_json.clone())
                .map_err(|error| format!("Invalid voice settings: {error}"))?;
            validate_voice_settings(&settings)
        }
        ("security.runtime", "default") => {
            let settings =
                serde_json::from_value::<SecurityRuntimeSettings>(input.value_json.clone())
                    .map_err(|error| format!("Invalid security settings: {error}"))?;
            validate_security_settings(&settings)
        }
        ("situation.runtime", "default") => {
            let settings =
                serde_json::from_value::<situation::contracts::SituationRuntimeSettings>(
                    input.value_json.clone(),
                )
                .map_err(|error| format!("Invalid Situation settings: {error}"))?;
            situation::validate_settings(&settings)
        }
        _ => Err("Unsupported settings document".to_string()),
    }
}

fn validate_settings_batch(documents: &[SaveSettingsDocumentInput]) -> Result<(), String> {
    if documents.len() != 6 {
        return Err("A complete six-document settings snapshot is required".to_string());
    }
    let unique = documents
        .iter()
        .map(|document| (document.namespace.as_str(), document.key.as_str()))
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != 6 {
        return Err("Each settings document must appear exactly once".to_string());
    }
    let providers = documents
        .iter()
        .find(|document| document.namespace == "providers.model" && document.key == "default")
        .ok_or_else(|| "Model provider settings are required".to_string())?;
    let routing = documents
        .iter()
        .find(|document| document.namespace == "routing.tasks" && document.key == "default")
        .ok_or_else(|| "Routing settings are required".to_string())?;
    let security = documents
        .iter()
        .find(|document| document.namespace == "security.runtime" && document.key == "default")
        .ok_or_else(|| "Security settings are required".to_string())?;
    let providers = serde_json::from_value::<ModelProvidersSettings>(providers.value_json.clone())
        .map_err(|error| format!("Invalid model provider settings: {error}"))?;
    let routing = serde_json::from_value::<RoutingSettings>(routing.value_json.clone())
        .map_err(|error| format!("Invalid routing settings: {error}"))?;
    let security = serde_json::from_value::<SecurityRuntimeSettings>(security.value_json.clone())
        .map_err(|error| format!("Invalid security settings: {error}"))?;
    let enabled_ids = providers
        .providers
        .iter()
        .filter(|provider| provider.enabled())
        .map(ModelProviderSettings::id)
        .collect::<std::collections::HashSet<_>>();
    if !enabled_ids.is_empty()
        && !enabled_ids.contains(routing.conversation_respond.primary_provider_id.as_str())
    {
        return Err("The primary conversation provider must be enabled".to_string());
    }
    let mut route_ids = std::collections::HashSet::new();
    route_ids.insert(routing.conversation_respond.primary_provider_id.as_str());
    for provider_id in &routing.conversation_respond.fallback_provider_ids {
        if !enabled_ids.contains(provider_id.as_str()) {
            return Err(format!("Fallback provider is not enabled: {provider_id}"));
        }
        if !route_ids.insert(provider_id) {
            return Err(format!("Duplicate provider in route: {provider_id}"));
        }
        let primary_is_local = providers.providers.iter().any(|provider| {
            provider.id() == routing.conversation_respond.primary_provider_id
                && provider.location() == "local"
        });
        let fallback_is_cloud = providers
            .providers
            .iter()
            .any(|provider| provider.id() == *provider_id && provider.location() == "cloud");
        if security.local_only_when_selected && primary_is_local && fallback_is_cloud {
            return Err(format!(
                "Cloud fallback is blocked while the local-only policy is active: {provider_id}"
            ));
        }
    }
    Ok(())
}

fn validate_model_providers(settings: &ModelProvidersSettings) -> Result<(), String> {
    if settings.providers.is_empty() || settings.providers.len() > 20 {
        return Err("Between 1 and 20 model providers are required".to_string());
    }
    let mut ids = std::collections::HashSet::new();
    let mut credential_suffixes = std::collections::HashSet::new();
    let mut enabled_larm_count = 0;
    for provider in &settings.providers {
        let provider_id = provider.id();
        if provider_id.is_empty()
            || provider_id.len() > 80
            || !provider_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
            || !ids.insert(provider_id)
            || !credential_suffixes.insert(provider_environment_suffix(provider_id))
        {
            return Err("Invalid, duplicate, or credential-ambiguous provider id".to_string());
        }
        if provider.label().trim().is_empty()
            || provider.label().len() > 120
            || provider.label().chars().any(char::is_control)
        {
            return Err(format!("Invalid provider label: {provider_id}"));
        }
        if !matches!(provider.location(), "local" | "cloud") {
            return Err(format!("Invalid provider location: {provider_id}"));
        }
        match provider {
            ModelProviderSettings::OpenAiCompatible(provider) => {
                if !matches!(
                    provider.credential_status.as_str(),
                    "not-configured" | "configured"
                ) {
                    return Err(format!("Invalid credential status: {provider_id}"));
                }
                if provider.endpoint.len() > 2_048
                    || provider.model.len() > 160
                    || provider.model.chars().any(char::is_control)
                {
                    return Err(format!(
                        "Provider endpoint or model is too long: {provider_id}"
                    ));
                }
                if provider.enabled
                    && (provider.endpoint.trim().is_empty() || provider.model.trim().is_empty())
                {
                    return Err(format!(
                        "Enabled provider requires endpoint and model: {provider_id}"
                    ));
                }
                if provider.endpoint.trim().is_empty() {
                    continue;
                }
                let endpoint = url::Url::parse(&provider.endpoint)
                    .map_err(|_| format!("Invalid provider endpoint: {provider_id}"))?;
                if !endpoint.username().is_empty() || endpoint.password().is_some() {
                    return Err(format!(
                        "Provider credentials must not be embedded in the endpoint: {provider_id}"
                    ));
                }
                if !matches!(endpoint.scheme(), "http" | "https") {
                    return Err(format!(
                        "Provider endpoint must use HTTP or HTTPS: {provider_id}"
                    ));
                }
                if provider.location == "local" {
                    if endpoint.scheme() != "http"
                        || !match endpoint.host() {
                            Some(url::Host::Domain(host)) => host == "localhost",
                            Some(url::Host::Ipv4(address)) => {
                                address.is_loopback() || address.is_private()
                            }
                            Some(url::Host::Ipv6(address)) => address.is_loopback(),
                            None => false,
                        }
                    {
                        return Err(format!(
                            "Local provider must use an HTTP loopback or private-network endpoint: {provider_id}"
                        ));
                    }
                } else if endpoint.scheme() != "https" {
                    return Err(format!("Cloud provider must use HTTPS: {provider_id}"));
                }
            }
            ModelProviderSettings::Larm(provider) => {
                if provider.enabled {
                    enabled_larm_count += 1;
                }
                if provider.location != "local"
                    || provider.base_url.len() > 2_048
                    || provider.token_env != "LARM_API_TOKEN"
                    || !(60..=3_600).contains(&provider.allocation_ttl_seconds)
                    || !(1..=300).contains(&provider.allocation_startup_timeout_seconds)
                    || provider.allow_fallback_by_default
                    || provider.deployment_policy != "existing-only"
                {
                    return Err(format!(
                        "LARM provider violates the fixed security policy: {provider_id}"
                    ));
                }
                let base_url = url::Url::parse(&provider.base_url)
                    .map_err(|_| format!("Invalid LARM base URL: {provider_id}"))?;
                let numeric_loopback = matches!(
                    base_url.host(),
                    Some(url::Host::Ipv4(address))
                        if address == std::net::Ipv4Addr::LOCALHOST
                ) || matches!(
                    base_url.host(),
                    Some(url::Host::Ipv6(address))
                        if address == std::net::Ipv6Addr::LOCALHOST
                );
                if base_url.scheme() != "http"
                    || !numeric_loopback
                    || base_url.port().is_none()
                    || !base_url.username().is_empty()
                    || base_url.password().is_some()
                    || base_url.query().is_some()
                    || base_url.fragment().is_some()
                    || base_url.path() != "/"
                {
                    return Err(format!(
                        "LARM base URL must be an explicit numeric HTTP loopback origin: {provider_id}"
                    ));
                }
            }
        }
    }
    if enabled_larm_count > 1 {
        return Err("Only one LARM provider may be enabled".to_string());
    }
    Ok(())
}

fn validate_codex_settings(settings: &CodexAgentRuntimeSettings) -> Result<(), String> {
    if settings.provider != "codex-sdk"
        || !matches!(
            settings.runtime_mode.as_str(),
            "pending-compatibility-check" | "bun" | "node-sidecar" | "app-server"
        )
        || !matches!(
            settings.health.as_str(),
            "unchecked" | "ready" | "unavailable"
        )
        || settings.sandbox_mode != "read-only"
        || settings.approval_policy != "never"
        || settings.network_enabled
        || settings.web_search_enabled
        || settings.workspace_policy != "select-per-conversation"
        || settings.model.len() > 160
        || settings.model.chars().any(char::is_control)
    {
        return Err("Codex settings violate the fixed read-only policy".to_string());
    }
    Ok(())
}

fn validate_routing_settings(settings: &RoutingSettings) -> Result<(), String> {
    let conversation = &settings.conversation_respond;
    let coding = &settings.coding_assist;
    if conversation.primary_provider_id.is_empty()
        || conversation.primary_provider_id.len() > 80
        || !conversation
            .primary_provider_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        || conversation.fallback_provider_ids.len() > 20
        || conversation
            .fallback_provider_ids
            .iter()
            .any(|provider_id| {
                provider_id.is_empty()
                    || provider_id.len() > 80
                    || !provider_id.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                    })
            })
        || !(1_000..=300_000).contains(&conversation.timeout_ms)
        || coding.provider_id != "codex-sdk"
        || !(1_000..=300_000).contains(&coding.timeout_ms)
        || !coding.read_only
        || coding.network_enabled
        || coding.web_search_enabled
    {
        return Err("Invalid task routing settings".to_string());
    }
    Ok(())
}

fn validate_voice_settings(settings: &VoiceRuntimeSettings) -> Result<(), String> {
    if settings.input_device_id.trim().is_empty()
        || settings.output_device_id.trim().is_empty()
        || settings.input_device_id.len() > 300
        || settings.output_device_id.len() > 300
        || settings.capture_mode != "push-to-talk"
        || settings.stt_provider_id != voice::gnosis_asr::PROVIDER_ID
        || settings.stt_model != voice::gnosis_asr::MODEL_ID
        || settings.tts_provider_id != "system-tts"
        || settings.tts_voice.trim().is_empty()
        || settings.tts_voice.len() > 160
        || settings.cloud_fallback_enabled
    {
        return Err("Invalid local voice settings".to_string());
    }
    Ok(())
}

fn validate_security_settings(settings: &SecurityRuntimeSettings) -> Result<(), String> {
    if settings.credential_storage != "environment" || !settings.diagnostics_redaction {
        return Err(
            "Secrets must remain outside SQLite and diagnostics must be redacted".to_string(),
        );
    }
    let _ = settings.local_only_when_selected;
    Ok(())
}

fn reconcile_interrupted_runs(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE runtime_runs
         SET status = 'interrupted', error_message = COALESCE(error_message, 'Application restarted'),
             failure_code = ?2,
             completed_at = ?1
         WHERE status = 'running'",
        params![
            now_iso(),
            runtime::contracts::RunFailureCode::AppRestarted.as_str()
        ],
    )?;
    connection.execute(
        "UPDATE provider_sessions
         SET status = 'interrupted', failure_reason = COALESCE(failure_reason, 'Application restarted'),
             release_status = CASE
               WHEN provider_kind='larm' AND allocation_id IS NOT NULL
                    AND release_status IN ('not-started','pending') THEN 'deferred-to-ttl'
               ELSE release_status
             END,
             updated_at = ?1
         WHERE status = 'running'",
        params![now_iso()],
    )?;
    Ok(())
}

fn read_settings_document(
    connection: &Connection,
    namespace: &str,
    key: &str,
) -> Result<SettingsDocument, String> {
    let document = connection
        .query_row(
            "SELECT namespace, key, schema_version, value_json, updated_at
             FROM settings_documents WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
            settings_document_from_row,
        )
        .map_err(database_error)?;
    validate_stored_settings_document(&document)?;
    Ok(document)
}

fn list_settings_documents(connection: &Connection) -> Result<Vec<SettingsDocument>, String> {
    let mut statement = connection
        .prepare(
            "SELECT namespace, key, schema_version, value_json, updated_at
             FROM settings_documents ORDER BY namespace, key",
        )
        .map_err(database_error)?;
    let documents = statement
        .query_map([], settings_document_from_row)
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let inputs = documents
        .iter()
        .map(|document| SaveSettingsDocumentInput {
            namespace: document.namespace.clone(),
            key: document.key.clone(),
            schema_version: document.schema_version,
            value_json: document.value_json.clone(),
        })
        .collect::<Vec<_>>();
    for input in &inputs {
        validate_settings_document(input)?;
    }
    validate_settings_batch(&inputs)?;
    Ok(documents)
}

fn validate_stored_settings_document(document: &SettingsDocument) -> Result<(), String> {
    validate_settings_document(&SaveSettingsDocumentInput {
        namespace: document.namespace.clone(),
        key: document.key.clone(),
        schema_version: document.schema_version,
        value_json: document.value_json.clone(),
    })
}

fn settings_document_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SettingsDocument> {
    let value_text: String = row.get(3)?;
    let value_json = serde_json::from_str(&value_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(SettingsDocument {
        namespace: row.get(0)?,
        key: row.get(1)?,
        schema_version: row.get(2)?,
        value_json,
        updated_at: row.get(4)?,
    })
}

fn list_conversations_from_connection(
    connection: &Connection,
) -> Result<Vec<Conversation>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, title, task_mode, created_at, updated_at
             FROM conversations
             WHERE task_mode = 'coding' OR id = ?1
             ORDER BY updated_at DESC LIMIT 30",
        )
        .map_err(database_error)?;
    let conversations = statement
        .query_map(params![PRIMARY_CONVERSATION_ID], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                task_mode: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for conversation in &conversations {
        validate_identifier(&conversation.id, "conversation id")?;
        if !matches!(conversation.task_mode.as_str(), "conversation" | "coding")
            || conversation
                .title
                .as_ref()
                .is_some_and(|title| title.chars().count() > 120)
            || conversation.created_at.parse::<u128>().is_err()
            || conversation.updated_at.parse::<u128>().is_err()
        {
            return Err("Invalid persisted conversation".to_string());
        }
    }
    Ok(conversations)
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
    fs::create_dir_all(&directory)?;
    Ok(directory.join("saaa.sqlite3"))
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
        .plugin(tauri_plugin_dialog::init())
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
                voice_data_directory,
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
                active_runs: Mutex::new(HashMap::new()),
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

    fn app_state(connection: Connection) -> AppState {
        let settings =
            situation::repository::load_settings(&connection).expect("Situation settings load");
        AppState {
            connection: Arc::new(Mutex::new(connection)),
            active_runs: Mutex::new(HashMap::new()),
            shutdown_started: AtomicBool::new(false),
            larm_gate: providers::larm::LarmRuntimeGate::Disabled,
            tts_process: Mutex::new(None),
            situation: Arc::new(
                situation::SituationRuntime::new(settings, None)
                    .expect("Situation runtime initializes"),
            ),
            meeting: Arc::new(meeting::MeetingRuntime::new()),
            voice_profile: Arc::new(voice::profile::VoiceProfileRuntime::unavailable_for_tests(
                PathBuf::new(),
            )),
        }
    }

    fn provider(id: &str, location: &str) -> ModelProviderSettings {
        ModelProviderSettings::OpenAiCompatible(direct_provider(id, location))
    }

    fn direct_provider(id: &str, location: &str) -> OpenAiCompatibleProviderSettings {
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
            credential_status: "not-configured".to_string(),
        }
    }

    fn larm_provider(id: &str) -> ModelProviderSettings {
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
    fn conversation_context_keeps_the_latest_hundred_messages_in_order() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, task_mode, created_at, updated_at)
                 VALUES ('conversation_history', 'conversation', '0', '0')",
                [],
            )
            .expect("conversation inserts");
        for index in 0..105 {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                     VALUES (?1, 'conversation_history', 'user', ?2, ?3)",
                    params![format!("message_{index}"), index.to_string(), index.to_string()],
                )
                .expect("message inserts");
        }
        let messages = list_messages_from_connection(&connection, "conversation_history")
            .expect("messages load");
        assert_eq!(messages.len(), 100);
        assert_eq!(messages.first().expect("first message").content, "5");
        assert_eq!(messages.last().expect("last message").content, "104");
    }

    #[test]
    fn primary_conversation_is_idempotent_and_preserves_legacy_history() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('legacy-conversation', 'Legacy', 'conversation', '0', '0')",
                [],
            )
            .expect("legacy conversation inserts");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('coding-conversation', 'Coding', 'coding', '0', '0')",
                [],
            )
            .expect("coding conversation inserts");

        let first = ensure_primary_conversation(&connection).expect("primary creates");
        let second = ensure_primary_conversation(&connection).expect("primary reuses");
        let primary_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id = ?1",
                params![PRIMARY_CONVERSATION_ID],
                |row| row.get(0),
            )
            .expect("primary count loads");
        let legacy_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id = 'legacy-conversation'",
                [],
                |row| row.get(0),
            )
            .expect("legacy count loads");
        let visible =
            list_conversations_from_connection(&connection).expect("visible conversations load");

        assert_eq!(first.id, PRIMARY_CONVERSATION_ID);
        assert_eq!(first.title.as_deref(), Some(PRIMARY_CONVERSATION_TITLE));
        assert_eq!(first.id, second.id);
        assert_eq!(primary_count, 1);
        assert_eq!(legacy_count, 1);
        assert!(visible
            .iter()
            .any(|conversation| conversation.id == PRIMARY_CONVERSATION_ID));
        assert!(visible
            .iter()
            .any(|conversation| conversation.id == "coding-conversation"));
        assert!(!visible
            .iter()
            .any(|conversation| conversation.id == "legacy-conversation"));
    }

    #[test]
    fn migration_creates_default_documents() {
        let connection = Connection::open_in_memory().expect("in-memory sqlite");
        initialize_database(&connection).expect("migration succeeds");
        let documents = list_settings_documents(&connection).expect("documents load");
        assert_eq!(documents.len(), 6);
        assert!(documents
            .iter()
            .all(|document| document.schema_version == 9));
        let providers = documents
            .iter()
            .find(|document| document.namespace == "providers.model")
            .expect("provider defaults exist");
        assert_eq!(
            providers.value_json.pointer("/providers/0/id"),
            Some(&json!(GNOSIS_PROVIDER_ID))
        );
        assert_eq!(
            providers.value_json.pointer("/providers/0/endpoint"),
            Some(&json!(GNOSIS_ENDPOINT))
        );
        assert_eq!(
            providers.value_json.pointer("/providers/0/model"),
            Some(&json!(GNOSIS_MODEL))
        );
        assert_eq!(
            providers.value_json.pointer("/providers/0/enabled"),
            Some(&json!(true))
        );
        let routing = documents
            .iter()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing defaults exist");
        assert_eq!(
            routing
                .value_json
                .pointer("/conversationRespond/primaryProviderId"),
            Some(&json!(GNOSIS_PROVIDER_ID))
        );
        let (version, active_profile): (i64, String) = (
            connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .expect("version reads"),
            connection
                .query_row(
                    "SELECT id FROM situation_calibration_profiles WHERE status='active'",
                    [],
                    |row| row.get(0),
                )
                .expect("active profile reads"),
        );
        assert_eq!(version, 11);
        assert_eq!(active_profile, "profile_mvp1_default");
        let recall_schema_objects: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE name IN (
                   'conversation_messages_fts',
                   'conversation_recall_cursors',
                   'conversation_recall_attempts',
                   'conversation_recall_receipts',
                   'conversation_messages_recall_insert',
                   'conversation_messages_recall_update',
                   'conversation_messages_recall_delete'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("recall schema reads");
        assert_eq!(recall_schema_objects, 7);
        let input_message_column: bool = connection
            .query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM pragma_table_info('runtime_runs') WHERE name='input_message_id'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("runtime input column reads");
        assert!(input_message_column);
    }

    #[test]
    fn pristine_previous_provider_defaults_migrate_to_gnosis() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        let legacy_providers = json!({
            "providers": [{
                "kind": "openai-compatible",
                "id": "local-openai-compatible",
                "enabled": false,
                "label": "Local OpenAI-compatible",
                "location": "local",
                "endpoint": "",
                "model": "",
                "credentialStatus": "not-configured"
            }]
        });
        let legacy_routing = json!({
            "conversationRespond": {
                "primaryProviderId": "local-openai-compatible",
                "fallbackProviderIds": [],
                "timeoutMs": 45000
            },
            "codingAssist": {
                "providerId": "codex-sdk",
                "timeoutMs": 120000,
                "readOnly": true,
                "networkEnabled": false,
                "webSearchEnabled": false
            }
        });
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1
                 WHERE namespace='providers.model' AND key='default'",
                [legacy_providers.to_string()],
            )
            .expect("legacy provider defaults write");
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1
                 WHERE namespace='routing.tasks' AND key='default'",
                [legacy_routing.to_string()],
            )
            .expect("legacy routing defaults write");

        initialize_database(&connection).expect("default upgrade succeeds");
        let documents = list_settings_documents(&connection).expect("settings load");
        let providers = documents
            .iter()
            .find(|document| document.namespace == "providers.model")
            .expect("providers exist");
        let routing = documents
            .iter()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing exists");
        assert_eq!(
            providers.value_json.pointer("/providers/0/id"),
            Some(&json!(GNOSIS_PROVIDER_ID))
        );
        assert_eq!(
            routing
                .value_json
                .pointer("/conversationRespond/primaryProviderId"),
            Some(&json!(GNOSIS_PROVIDER_ID))
        );
        assert_eq!(
            routing.value_json.pointer("/conversationRespond/timeoutMs"),
            Some(&json!(45000))
        );
    }

    #[test]
    fn version_eight_voice_and_meeting_schema_migrate_to_gnosis_asr() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection
            .execute(
                "UPDATE settings_documents
                 SET schema_version=8,
                     value_json=json_set(value_json,
                       '$.sttProviderId','local-whisper',
                       '$.sttModel','/tmp/legacy-model.bin')
                 WHERE namespace='voice.runtime' AND key='default'",
                [],
            )
            .expect("v8 voice settings write");
        connection
            .execute_batch(
                "DROP TABLE meeting_transcript_entries;
                 DROP TABLE meeting_sessions;
                 CREATE TABLE meeting_sessions (
                   id TEXT PRIMARY KEY,
                   status TEXT NOT NULL CHECK(status IN ('active','paused','completed','saved','discarded','failed','interrupted')),
                   microphone_enabled INTEGER NOT NULL CHECK(microphone_enabled IN (0,1)),
                   system_audio_enabled INTEGER NOT NULL CHECK(system_audio_enabled IN (0,1)),
                   stt_provider_id TEXT NOT NULL CHECK(stt_provider_id = 'local-whisper'),
                   stt_model_label TEXT NOT NULL CHECK(length(stt_model_label) <= 256),
                   translation_provider_id TEXT,
                   persistence_mode TEXT NOT NULL CHECK(persistence_mode IN ('discard','explicit-save')),
                   started_at TEXT NOT NULL, ended_at TEXT, saved_at TEXT, error_code TEXT
                 );
                 CREATE TABLE meeting_transcript_entries (
                   id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                   lane TEXT NOT NULL CHECK(lane IN ('microphone','system-audio')),
                   sequence INTEGER NOT NULL CHECK(sequence >= 0),
                   original_text TEXT NOT NULL CHECK(length(original_text) BETWEEN 1 AND 8000),
                   original_language TEXT,
                   translated_text TEXT CHECK(translated_text IS NULL OR length(translated_text) <= 8000),
                   translated_language TEXT,
                   started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0),
                   ended_at_ms INTEGER NOT NULL CHECK(ended_at_ms >= started_at_ms),
                   created_at TEXT NOT NULL,
                   FOREIGN KEY(session_id) REFERENCES meeting_sessions(id) ON DELETE CASCADE,
                   UNIQUE(session_id,lane,sequence)
                 );
                 INSERT INTO meeting_sessions(
                   id,status,microphone_enabled,system_audio_enabled,stt_provider_id,
                   stt_model_label,persistence_mode,started_at,ended_at
                 ) VALUES(
                   'legacy-meeting','completed',1,0,'local-whisper','legacy-model.bin',
                   'discard','1','2'
                 );
                 INSERT INTO meeting_transcript_entries(
                   id,session_id,lane,sequence,original_text,started_at_ms,ended_at_ms,created_at
                 ) VALUES(
                   'legacy-entry','legacy-meeting','microphone',0,'kept transcript',0,1000,'2'
                 );",
            )
            .expect("v8 meeting schema writes");
        connection
            .pragma_update(None, "user_version", 8)
            .expect("v8 version writes");

        initialize_database(&connection).expect("v9 migration succeeds");

        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version reads");
        let voice: String = connection
            .query_row(
                "SELECT value_json FROM settings_documents
                 WHERE namespace='voice.runtime' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("voice settings read");
        let voice: Value = serde_json::from_str(&voice).expect("voice settings decode");
        let meeting_schema: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='meeting_sessions'",
                [],
                |row| row.get(0),
            )
            .expect("meeting schema reads");
        let transcript: String = connection
            .query_row(
                "SELECT original_text FROM meeting_transcript_entries
                 WHERE id='legacy-entry'",
                [],
                |row| row.get(0),
            )
            .expect("legacy transcript remains");
        assert_eq!(version, 11);
        assert_eq!(
            voice.pointer("/sttProviderId"),
            Some(&json!(voice::gnosis_asr::PROVIDER_ID))
        );
        assert_eq!(
            voice.pointer("/sttModel"),
            Some(&json!(voice::gnosis_asr::MODEL_ID))
        );
        assert!(meeting_schema.contains("gnosis-asr"));
        assert_eq!(transcript, "kept transcript");
        connection
            .execute("DELETE FROM meeting_sessions WHERE id='legacy-meeting'", [])
            .expect("cascading delete succeeds");
        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM meeting_transcript_entries WHERE id='legacy-entry'",
                [],
                |row| row.get(0),
            )
            .expect("cascade count reads");
        assert_eq!(remaining, 0);
    }

    #[test]
    fn version_five_database_migrates_to_eight_without_losing_existing_data() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection.execute("INSERT INTO conversations(id,title,task_mode,created_at,updated_at) VALUES('kept','Keep','conversation','1','1')", []).expect("conversation persists");
        connection
            .execute("UPDATE settings_documents SET schema_version=5", [])
            .expect("v5 settings fixture");
        connection
            .pragma_update(None, "user_version", 5)
            .expect("v5 fixture");
        initialize_database(&connection).expect("v8 migration");
        let conversation: String = connection
            .query_row(
                "SELECT title FROM conversations WHERE id='kept'",
                [],
                |row| row.get(0),
            )
            .expect("conversation retained");
        assert_eq!(conversation, "Keep");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version reads");
        assert_eq!(version, 11);
        assert!(connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meeting_transcript_entries')", [], |row| row.get::<_, bool>(0)).expect("meeting table exists"));
        initialize_database(&connection).expect("migration idempotent");
    }

    #[test]
    fn version_six_calibration_is_inherited_with_new_input_defaults() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        let legacy_json = r#"{"classificationMinConfidence":75,"lowConfidenceMax":40,"enterSampleCount":4,"exitSampleCount":6,"cooldownMs":12000}"#;
        connection
            .execute(
                "UPDATE situation_calibration_profiles
                 SET parameters_json=?1
                 WHERE id='profile_mvp1_default'",
                [legacy_json],
            )
            .expect("legacy profile writes");
        connection
            .pragma_update(None, "user_version", 6)
            .expect("v6 fixture");

        initialize_database(&connection).expect("v8 migration");
        let (rule_version, parameters_json): (String, String) = connection
            .query_row(
                "SELECT rule_version,parameters_json
                 FROM situation_calibration_profiles
                 WHERE id='profile_mvp1_default' AND status='active'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("migrated profile reads");
        let parameters: situation::contracts::CalibrationParameters =
            serde_json::from_str(&parameters_json).expect("parameters decode");
        assert_eq!(rule_version, "mvp1-rules-v1");
        assert_eq!(parameters_json, legacy_json);
        assert_eq!(parameters.classification_min_confidence, 75);
        assert_eq!(parameters.enter_sample_count, 4);
        assert_eq!(parameters.input_active_max_ms, 30_000);
        assert_eq!(parameters.input_recent_max_ms, 300_000);
        initialize_database(&connection).expect("legacy profile remains readable after reopen");
    }

    #[test]
    fn version_six_settings_are_normalized_to_the_strict_v8_shape() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection
            .execute(
                "UPDATE settings_documents
                 SET schema_version=6,
                     value_json=json_set(value_json, '$.legacyField', 'ignored')",
                [],
            )
            .expect("legacy top-level fields write");
        connection
            .execute(
                "UPDATE settings_documents
                 SET value_json=json_set(value_json, '$.providers[0].legacyProviderField', 1)
                 WHERE namespace='providers.model'",
                [],
            )
            .expect("legacy nested field writes");
        connection
            .pragma_update(None, "user_version", 6)
            .expect("v6 fixture");

        initialize_database(&connection).expect("v8 migration");
        let documents = list_settings_documents(&connection).expect("strict settings load");
        assert_eq!(documents.len(), 6);
        assert!(documents.iter().all(|document| {
            document.schema_version == 9 && document.value_json.get("legacyField").is_none()
        }));
        let providers = documents
            .iter()
            .find(|document| document.namespace == "providers.model")
            .and_then(|document| document.value_json.pointer("/providers/0"))
            .expect("provider remains");
        assert!(providers.get("legacyProviderField").is_none());
        assert_eq!(providers.get("kind"), Some(&json!("openai-compatible")));
    }

    #[test]
    fn version_seven_settings_and_provider_sessions_migrate_to_v8() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        let v7_providers = json!({
            "providers": [{
                "id": "provider-a", "enabled": true, "label": "Provider A", "location": "local",
                "endpoint": "http://127.0.0.1:11434/v1", "model": "kept-model", "credentialStatus": "not-configured"
            }, {
                "id": "provider-b", "enabled": true, "label": "Provider B", "location": "local",
                "endpoint": "http://127.0.0.1:11435/v1", "model": "model-b", "credentialStatus": "not-configured"
            }, {
                "id": "provider-c", "enabled": true, "label": "Provider C", "location": "local",
                "endpoint": "http://127.0.0.1:11436/v1", "model": "model-c", "credentialStatus": "not-configured"
            }]
        });
        connection
            .execute(
                "UPDATE settings_documents
                 SET schema_version=7, value_json=?1
                 WHERE namespace='providers.model' AND key='default'",
                [v7_providers.to_string()],
            )
            .expect("v7 provider fixture writes");
        let v7_routing = json!({
            "conversationRespond": {
                "primaryProviderId": "provider-a",
                "fallbackProviderIds": ["provider-b", "provider-c"],
                "timeoutMs": 30_000
            },
            "codingAssist": {
                "providerId": "codex-sdk", "timeoutMs": 120_000, "readOnly": true,
                "networkEnabled": false, "webSearchEnabled": false
            }
        });
        connection
            .execute(
                "UPDATE settings_documents SET schema_version=7, value_json=?1
                 WHERE namespace='routing.tasks' AND key='default'",
                [v7_routing.to_string()],
            )
            .expect("v7 routing fixture writes");
        connection
            .execute("UPDATE settings_documents SET schema_version=7", [])
            .expect("v7 settings fixture writes");
        connection
            .execute_batch(
                "DROP INDEX idx_provider_sessions_runtime_run;
                 ALTER TABLE provider_sessions DROP COLUMN runtime_run_id;
                 ALTER TABLE provider_sessions DROP COLUMN provider_kind;
                 ALTER TABLE provider_sessions DROP COLUMN route_id;
                 ALTER TABLE provider_sessions DROP COLUMN allocation_id;
                 ALTER TABLE provider_sessions DROP COLUMN selected_runtime_id;
                 ALTER TABLE provider_sessions DROP COLUMN fallback_used;
                 ALTER TABLE provider_sessions DROP COLUMN selection_reason;
                 ALTER TABLE provider_sessions DROP COLUMN request_id;
                 ALTER TABLE provider_sessions DROP COLUMN output_started;
                 ALTER TABLE provider_sessions DROP COLUMN failure_kind;
                 ALTER TABLE provider_sessions DROP COLUMN release_status;
                 ALTER TABLE provider_sessions DROP COLUMN release_failure_kind;",
            )
            .expect("v7 provider session shape restores");
        connection
            .execute(
                "INSERT INTO provider_sessions(
                   id, provider_id, status, started_at, updated_at
                 ) VALUES('legacy-session', 'legacy-provider', 'completed', '1', '1')",
                [],
            )
            .expect("v7 provider session row writes");
        connection
            .pragma_update(None, "user_version", 7)
            .expect("v7 fixture");

        initialize_database(&connection).expect("v8 migration succeeds");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version reads");
        let provider_value: String = connection
            .query_row(
                "SELECT value_json FROM settings_documents
                 WHERE namespace='providers.model' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("provider settings read");
        let provider_value: Value =
            serde_json::from_str(&provider_value).expect("provider settings decode");
        assert_eq!(version, 11);
        assert_eq!(
            provider_value.pointer("/providers/0/kind"),
            Some(&json!("openai-compatible"))
        );
        assert_eq!(
            provider_value.pointer("/providers/0/model"),
            Some(&json!("kept-model"))
        );
        let provider_ids = provider_value["providers"]
            .as_array()
            .expect("provider list remains an array")
            .iter()
            .map(|provider| {
                provider["id"]
                    .as_str()
                    .expect("provider id remains a string")
            })
            .collect::<Vec<_>>();
        assert_eq!(provider_ids, ["provider-a", "provider-b", "provider-c"]);
        let routing_value: String = connection
            .query_row(
                "SELECT value_json FROM settings_documents
                 WHERE namespace='routing.tasks' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("routing settings read");
        let routing_value: Value =
            serde_json::from_str(&routing_value).expect("routing settings decode");
        assert_eq!(
            routing_value.pointer("/conversationRespond/fallbackProviderIds"),
            Some(&json!(["provider-b", "provider-c"]))
        );
        for column in [
            "runtime_run_id",
            "provider_kind",
            "allocation_id",
            "selected_runtime_id",
            "output_started",
            "release_status",
        ] {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('provider_sessions') WHERE name=?1)",
                    [column],
                    |row| row.get(0),
                )
                .expect("provider session column check succeeds");
            assert!(exists, "missing provider session column: {column}");
        }
        let (runtime_run_id, fallback_used, output_started, release_status): (
            Option<String>,
            Option<bool>,
            Option<bool>,
            String,
        ) = connection
            .query_row(
                "SELECT runtime_run_id, fallback_used, output_started, release_status
                 FROM provider_sessions WHERE id='legacy-session'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("legacy provider session reads");
        assert!(runtime_run_id.is_none());
        assert!(fallback_used.is_none());
        assert!(output_started.is_none());
        assert_eq!(release_status, "not-applicable");
        assert!(connection
            .execute(
                "INSERT INTO provider_sessions(
                   id, provider_id, runtime_run_id, provider_kind, status, started_at, updated_at
                 ) VALUES('invalid-session', 'provider', 'invalid run id', 'larm', 'failed', '1', '1')",
                [],
            )
            .is_err());
        connection
            .execute(
                "INSERT INTO provider_sessions(
                   id, provider_id, runtime_run_id, provider_kind, route_id, selection_reason,
                   release_status, status, started_at, updated_at
                 ) VALUES(
                   'bounded-session', 'provider', 'run_1', 'larm', 'llm-default', 'other',
                   'deferred-to-ttl', 'completed', '1', '1'
                 )",
                [],
            )
            .expect("bounded v8 provider session row writes");
        initialize_database(&connection).expect("v8 migration is idempotent");
    }

    #[test]
    fn version_six_database_is_backed_up_before_v8() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("v6.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
                 VALUES('backup-kept','Keep','coding','1','1')",
                [],
            )
            .expect("fixture inserts");
        connection
            .pragma_update(None, "user_version", 6)
            .expect("v6 fixture");
        drop(connection);

        let connection = Connection::open(&path).expect("database reopens");
        let backup = backup_before_migration(&connection, &path)
            .expect("backup succeeds")
            .expect("v6 backup is created");
        initialize_database(&connection).expect("v8 migration succeeds");
        let backup_connection = Connection::open(backup).expect("backup reopens");
        let title: String = backup_connection
            .query_row(
                "SELECT title FROM conversations WHERE id='backup-kept'",
                [],
                |row| row.get(0),
            )
            .expect("backup data remains");
        let backup_version: i64 = backup_connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("backup version reads");
        assert_eq!(title, "Keep");
        assert_eq!(backup_version, 6);
    }

    #[test]
    fn version_seven_database_is_backed_up_before_v8() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("v7.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "UPDATE settings_documents
                 SET schema_version=7,
                     value_json=json_remove(value_json, '$.providers[0].kind')
                 WHERE namespace='providers.model'",
                [],
            )
            .expect("v7 provider fixture writes");
        connection
            .execute("UPDATE settings_documents SET schema_version=7", [])
            .expect("v7 settings fixture writes");
        connection
            .pragma_update(None, "user_version", 7)
            .expect("v7 fixture");
        drop(connection);

        let connection = Connection::open(&path).expect("database reopens");
        let backup = backup_before_migration(&connection, &path)
            .expect("backup succeeds")
            .expect("v7 backup is created");
        initialize_database(&connection).expect("v8 migration succeeds");
        let backup_connection = Connection::open(backup).expect("backup reopens");
        let backup_version: i64 = backup_connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("backup version reads");
        let backup_provider_kind: Option<String> = backup_connection
            .query_row(
                "SELECT json_extract(value_json, '$.providers[0].kind')
                 FROM settings_documents
                 WHERE namespace='providers.model' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("backup provider settings read");
        assert_eq!(backup_version, 7);
        assert!(backup_provider_kind.is_none());
        drop(backup_connection);
        drop(connection);

        let reopened = Connection::open(&path).expect("migrated database reopens");
        initialize_database(&reopened).expect("reopened v8 database validates");
        let migrated_provider_kind: String = reopened
            .query_row(
                "SELECT json_extract(value_json, '$.providers[0].kind')
                 FROM settings_documents
                 WHERE namespace='providers.model' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("migrated provider settings read");
        assert_eq!(migrated_provider_kind, "openai-compatible");
    }

    #[test]
    fn version_six_runtime_rows_gain_nullable_supervisor_columns() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute_batch(
                "ALTER TABLE runtime_runs DROP COLUMN failure_code;
                 ALTER TABLE runtime_runs DROP COLUMN supervisor_version;
                 ALTER TABLE runtime_runs DROP COLUMN last_progress_at;",
            )
            .expect("v6 columns remove");
        connection
            .pragma_update(None, "user_version", 6)
            .expect("v6 fixture");
        initialize_database(&connection).expect("v8 migration succeeds");
        for column in ["failure_code", "supervisor_version", "last_progress_at"] {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('runtime_runs') WHERE name=?1)",
                    [column],
                    |row| row.get(0),
                )
                .expect("column check succeeds");
            assert!(exists, "missing column: {column}");
        }
        connection
            .execute(
                "INSERT INTO conversations(id,task_mode,created_at,updated_at)
                 VALUES('nullable','coding','1','1')",
                [],
            )
            .expect("conversation inserts");
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at)
                 VALUES('nullable-run','nullable','coding.assist','running','1')",
                [],
            )
            .expect("nullable migrated columns accept old rows");
    }

    #[test]
    fn startup_reconciles_unfinished_meeting_without_persisted_transcript() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection.execute("INSERT INTO meeting_sessions(id,status,microphone_enabled,system_audio_enabled,stt_provider_id,stt_model_label,persistence_mode,started_at) VALUES('meeting_recover','active',1,0,'local-whisper','model.bin','discard','1')", []).expect("meeting fixture");
        initialize_database(&connection).expect("startup reconciliation");
        let status: String = connection
            .query_row(
                "SELECT status FROM meeting_sessions WHERE id='meeting_recover'",
                [],
                |row| row.get(0),
            )
            .expect("status reads");
        let transcript_count: i64 = connection.query_row("SELECT COUNT(*) FROM meeting_transcript_entries WHERE session_id='meeting_recover'", [], |row| row.get(0)).expect("transcript count");
        assert_eq!(status, "interrupted");
        assert_eq!(transcript_count, 0);
    }

    #[test]
    fn settings_survive_reopen_and_invalid_batch_does_not_replace_snapshot() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.sqlite3");
        let mut connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut documents = default_settings_input();
        let security = documents
            .iter_mut()
            .find(|document| document.namespace == "security.runtime")
            .expect("security document");
        security.value_json["localOnlyWhenSelected"] = Value::Bool(false);
        save_settings_documents_to_connection(&mut connection, &documents)
            .expect("valid settings save");
        drop(connection);

        let mut reopened = Connection::open(&path).expect("database reopens");
        initialize_database(&reopened).expect("database reinitializes");
        let loaded = list_settings_documents(&reopened).expect("settings reload");
        let security = loaded
            .iter()
            .find(|document| document.namespace == "security.runtime")
            .expect("security settings reload");
        assert_eq!(security.value_json["localOnlyWhenSelected"], false);

        let mut invalid = default_settings_input();
        invalid
            .iter_mut()
            .find(|document| document.namespace == "security.runtime")
            .expect("security document")
            .value_json["credentialStorage"] = Value::String("plaintext".to_string());
        assert!(save_settings_documents_to_connection(&mut reopened, &invalid).is_err());
        let unchanged = read_settings_document(&reopened, "security.runtime", "default")
            .expect("previous snapshot remains");
        assert_eq!(unchanged.value_json["localOnlyWhenSelected"], false);
    }

    #[test]
    fn version_three_database_migrates_to_four_without_losing_mvp_zero_state() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("v3.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("initial schema creates");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('kept-conversation', 'Keep me', 'coding', '1', '1')",
                [],
            )
            .expect("conversation inserts");
        connection
            .execute(
                "INSERT INTO codex_threads(conversation_id, thread_id, model, workspace_path, updated_at)
                 VALUES ('kept-conversation', 'kept-thread', 'kept-model', '/tmp/kept', '1')",
                [],
            )
            .expect("thread inserts");
        connection
            .execute(
                "DELETE FROM settings_documents WHERE namespace = 'situation.runtime'",
                [],
            )
            .expect("v4 document removes");
        connection
            .execute("UPDATE settings_documents SET schema_version = 3", [])
            .expect("settings downgrade fixture");
        connection
            .pragma_update(None, "user_version", 3)
            .expect("fixture version sets");
        drop(connection);

        let reopened = Connection::open(&path).expect("database reopens");
        initialize_database(&reopened).expect("v4 migration succeeds");
        let version: i64 = reopened
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version reads");
        assert_eq!(version, 11);
        let documents = list_settings_documents(&reopened).expect("settings load");
        assert_eq!(documents.len(), 6);
        assert!(documents
            .iter()
            .all(|document| document.schema_version == 9));
        let thread: String = reopened
            .query_row(
                "SELECT thread_id FROM codex_threads WHERE conversation_id = 'kept-conversation'",
                [],
                |row| row.get(0),
            )
            .expect("thread remains");
        assert_eq!(thread, "kept-thread");
    }

    #[test]
    fn startup_reconciles_running_work() {
        let connection = Connection::open_in_memory().expect("in-memory sqlite");
        initialize_database(&connection).expect("migration succeeds");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('conversation-1', NULL, 'conversation', 'now', 'now')",
                [],
            )
            .expect("conversation inserts");
        connection
            .execute(
                "INSERT INTO runtime_runs(id, conversation_id, route_kind, status, started_at)
                 VALUES ('run-1', 'conversation-1', 'conversation.respond', 'running', 'before-restart')",
                [],
            )
            .expect("running work inserts");
        connection
            .execute(
                "INSERT INTO provider_sessions(
                   id, provider_id, runtime_run_id, provider_kind, allocation_id,
                   fallback_used, output_started, release_status, status, started_at, updated_at
                 ) VALUES(
                   'session-1','larm-primary','run-1','larm','alloc_restart',
                   0,1,'pending','running','before-restart','before-restart'
                 )",
                [],
            )
            .expect("running provider session inserts");

        reconcile_interrupted_runs(&connection).expect("startup reconciliation succeeds");
        let (status, failure_code, supervisor_version): (String, String, Option<String>) =
            connection
                .query_row(
                    "SELECT status,failure_code,supervisor_version
                 FROM runtime_runs WHERE id = 'run-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("run status loads");
        assert_eq!(status, "interrupted");
        assert_eq!(failure_code, "app-restarted");
        assert_eq!(supervisor_version, None);
        let (provider_status, release_status): (String, String) = connection
            .query_row(
                "SELECT status,release_status FROM provider_sessions WHERE id='session-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("provider session loads");
        assert_eq!(provider_status, "interrupted");
        assert_eq!(release_status, "deferred-to-ttl");
    }

    #[test]
    fn provider_diagnostics_exclude_allocation_and_request_identifiers() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO provider_sessions(
                   id,provider_id,runtime_run_id,provider_kind,route_id,allocation_id,
                   selected_runtime_id,fallback_used,selection_reason,request_id,output_started,
                   release_status,status,started_at,updated_at
                 ) VALUES(
                   'session_diag','larm-primary','run_diag','larm','llm-default','alloc_secret',
                   'runtime_safe',0,'primary','req_secret',1,'released','completed','1','2'
                 )",
                [],
            )
            .expect("diagnostic fixture inserts");
        let diagnostics = build_provider_diagnostics(&connection).expect("diagnostics build");
        let encoded = diagnostics.to_string();
        assert!(encoded.contains("runtime_safe"));
        assert!(encoded.contains("llm-default"));
        for forbidden in ["alloc_secret", "req_secret", "allocationId", "requestId"] {
            assert!(
                !encoded.contains(forbidden),
                "diagnostics exposed {forbidden}"
            );
        }
    }

    #[test]
    fn local_only_route_excludes_cloud_fallback() {
        let providers = ModelProvidersSettings {
            providers: vec![
                provider("local-primary", "local"),
                provider("cloud-fallback", "cloud"),
                provider("local-fallback", "local"),
            ],
        };
        let route = ConversationRouteSettings {
            primary_provider_id: "local-primary".to_string(),
            fallback_provider_ids: vec!["cloud-fallback".to_string(), "local-fallback".to_string()],
            timeout_ms: 30_000,
        };
        let security = SecurityRuntimeSettings {
            credential_storage: "environment".to_string(),
            local_only_when_selected: true,
            diagnostics_redaction: true,
        };

        assert_eq!(
            effective_conversation_route_ids(&providers, &route, &security),
            ["local-primary", "local-fallback"]
        );
    }

    #[test]
    fn disabled_larm_gate_removes_only_larm_and_preserves_direct_rollback_order() {
        let providers = ModelProvidersSettings {
            providers: vec![
                larm_provider("larm-primary"),
                provider("direct-rollback", "local"),
            ],
        };
        let configured = vec!["larm-primary".to_string(), "direct-rollback".to_string()];
        assert_eq!(
            apply_runtime_provider_gates(
                &providers,
                configured.clone(),
                &providers::larm::LarmRuntimeGate::Disabled,
            ),
            vec!["direct-rollback"]
        );
        let ready = providers::larm::LarmRuntimeGate::Ready(Arc::new(
            providers::larm::client::SharedLarmClient::build().expect("LARM client builds"),
        ));
        assert_eq!(
            apply_runtime_provider_gates(&providers, configured.clone(), &ready),
            configured
        );
    }

    #[test]
    fn settings_reject_embedded_credentials_and_cloud_fallback_on_local_route() {
        let mut gnosis = direct_provider(GNOSIS_PROVIDER_ID, "local");
        gnosis.endpoint = GNOSIS_ENDPOINT.to_string();
        gnosis.model = GNOSIS_MODEL.to_string();
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(gnosis)]
        })
        .is_ok());
        let mut public_http = direct_provider("public-http", "local");
        public_http.endpoint = "http://203.0.113.10:8080/v1".to_string();
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(public_http)]
        })
        .is_err());

        let with_credentials = ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(
                OpenAiCompatibleProviderSettings {
                    endpoint: "https://user:secret@example.invalid/v1".to_string(),
                    ..direct_provider("cloud", "cloud")
                },
            )],
        };
        assert!(validate_model_providers(&with_credentials).is_err());
        let mut disabled_with_credentials = with_credentials;
        let ModelProviderSettings::OpenAiCompatible(disabled_provider) =
            &mut disabled_with_credentials.providers[0]
        else {
            unreachable!("fixture is direct provider");
        };
        disabled_provider.enabled = false;
        assert!(validate_model_providers(&disabled_with_credentials).is_err());

        let unsafe_id = ModelProvidersSettings {
            providers: vec![provider("local provider", "local")],
        };
        assert!(validate_model_providers(&unsafe_id).is_err());
        let ambiguous_ids = ModelProvidersSettings {
            providers: vec![provider("local-a", "local"), provider("local_a", "local")],
        };
        assert!(validate_model_providers(&ambiguous_ids).is_err());

        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings")
            .value_json = json!({ "providers": [
            provider("local", "local"),
            provider("cloud", "cloud")
        ]});
        let routing = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        routing.value_json["conversationRespond"]["primaryProviderId"] = json!("local");
        routing.value_json["conversationRespond"]["fallbackProviderIds"] = json!(["cloud"]);
        assert!(validate_settings_batch(&documents).is_err());
    }

    #[test]
    fn larm_settings_enforce_the_fixed_loopback_security_contract() {
        let valid = ModelProvidersSettings {
            providers: vec![larm_provider("larm")],
        };
        assert!(validate_model_providers(&valid).is_ok());
        let mut ipv6 = larm_provider("larm-ipv6");
        let ModelProviderSettings::Larm(provider) = &mut ipv6 else {
            unreachable!("LARM fixture must remain tagged as LARM");
        };
        provider.base_url = "http://[::1]:9810/".to_string();
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ipv6]
        })
        .is_ok());

        for base_url in [
            "http://localhost:9810/",
            "http://192.168.1.20:9810/",
            "https://127.0.0.1:9810/",
            "http://127.0.0.1:9810/v1",
            "http://user:secret@127.0.0.1:9810/",
            "http://127.0.0.1/",
        ] {
            let mut invalid = larm_provider("larm");
            let ModelProviderSettings::Larm(provider) = &mut invalid else {
                unreachable!("LARM fixture must remain tagged as LARM");
            };
            provider.base_url = base_url.to_string();
            assert!(
                validate_model_providers(&ModelProvidersSettings {
                    providers: vec![invalid]
                })
                .is_err(),
                "invalid LARM URL was accepted: {base_url}"
            );
        }

        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![larm_provider("larm-a"), larm_provider("larm-b")],
        })
        .is_err());
    }

    #[test]
    fn legacy_provider_ids_and_default_codex_model_remain_valid() {
        let providers = ModelProvidersSettings {
            providers: vec![provider("Local_Custom", "local")],
        };
        assert!(validate_model_providers(&providers).is_ok());

        let codex = CodexAgentRuntimeSettings {
            enabled: true,
            provider: "codex-sdk".to_string(),
            model: String::new(),
            runtime_mode: "app-server".to_string(),
            health: "unchecked".to_string(),
            sandbox_mode: "read-only".to_string(),
            approval_policy: "never".to_string(),
            network_enabled: false,
            web_search_enabled: false,
            workspace_policy: "select-per-conversation".to_string(),
        };
        assert!(validate_codex_settings(&codex).is_ok());
    }

    #[test]
    fn sse_parser_handles_lf_crlf_and_partial_events() {
        let mut buffer = b"data: {\"a\":1}\r\n\r\ndata: {\"b\":2}\n\ndata: partial".to_vec();
        let events = drain_sse_events(&mut buffer, 1_048_576).expect("SSE events parse");
        assert_eq!(events, ["data: {\"a\":1}", "data: {\"b\":2}"]);
        assert_eq!(buffer, b"data: partial");
    }

    #[test]
    fn sse_parser_applies_the_limit_to_each_event() {
        let event = format!("data: {}\n\n", "x".repeat(600 * 1_024));
        let mut buffer = [event.as_bytes(), event.as_bytes()].concat();
        let events = drain_sse_events(&mut buffer, 1_048_576)
            .expect("two individually bounded events parse");
        assert_eq!(events.len(), 2);
        assert!(buffer.is_empty());

        let mut oversized = format!("data: {}\n\n", "x".repeat(1_048_577)).into_bytes();
        assert_eq!(
            drain_sse_events(&mut oversized, 1_048_576),
            Err(SseDrainError::EventTooLarge)
        );
        let mut invalid_utf8 = b"data: \xff\n\n".to_vec();
        assert_eq!(
            drain_sse_events(&mut invalid_utf8, 1_048_576),
            Err(SseDrainError::InvalidUtf8)
        );
    }

    #[test]
    fn sse_parser_preserves_utf8_split_across_network_chunks() {
        let event = "data: {\"choices\":[{\"delta\":{\"content\":\"こんにちは\"}}]}\n\n";
        let bytes = event.as_bytes();
        let split = bytes
            .windows("こ".len())
            .position(|window| window == "こ".as_bytes())
            .expect("multibyte character exists")
            + 1;
        let mut buffer = bytes[..split].to_vec();
        assert!(drain_sse_events(&mut buffer, 1_048_576)
            .expect("partial UTF-8 remains buffered")
            .is_empty());
        buffer.extend_from_slice(&bytes[split..]);
        assert_eq!(
            drain_sse_events(&mut buffer, 1_048_576).expect("complete UTF-8 event parses"),
            [event.trim_end()]
        );
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
        let content = stream_model_provider(
            &provider,
            &history,
            5_000,
            &input,
            &channel,
            Arc::new(RunCancellation::default()),
            None,
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
                 INSERT INTO conversations(id,task_mode,created_at,updated_at)
                   VALUES('conversation-current','conversation','2','2');
                 INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                   VALUES('message-old-user','conversation-old','user','SQLite の検索方式を相談した','1000');
                 INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                   VALUES('message-old-assistant','conversation-old','assistant','FTS と時間条件を組み合わせます','1001');",
            )
            .expect("history inserts");
        let state = app_state(connection);
        let input = StartTurnInput {
            run_id: "run-recall-tool".to_string(),
            conversation_id: "conversation-current".to_string(),
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
            &input,
            &channel,
            Arc::new(RunCancellation::default()),
            Some(ProviderOutputPersistence {
                state: &state,
                session_id: &session_id,
            }),
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

    #[test]
    fn malformed_recall_calls_consume_the_persistent_turn_limit() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id,task_mode,created_at,updated_at)
                 VALUES('conversation-malformed','conversation','1','1')",
                [],
            )
            .expect("conversation inserts");
        let state = app_state(connection);
        let input = StartTurnInput {
            run_id: "run-malformed-recall".to_string(),
            conversation_id: "conversation-malformed".to_string(),
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
        connection
            .execute(
                "INSERT INTO conversations(id,task_mode,created_at,updated_at)
                 VALUES('conversation-timeout','conversation','1','1')",
                [],
            )
            .expect("conversation inserts");
        let state = app_state(connection);
        let input = StartTurnInput {
            run_id: "run-recall-timeout".to_string(),
            conversation_id: "conversation-timeout".to_string(),
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
            &input,
            &channel,
            Arc::new(RunCancellation::default()),
            Some(ProviderOutputPersistence {
                state: &state,
                session_id: &session_id,
            }),
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

    #[test]
    fn provider_fallback_policy_is_failure_kind_and_output_aware() {
        for kind in [
            ProviderFailureKind::Policy,
            ProviderFailureKind::Capacity,
            ProviderFailureKind::Unavailable,
            ProviderFailureKind::Draining,
            ProviderFailureKind::Upstream,
            ProviderFailureKind::Network,
            ProviderFailureKind::Timeout,
            ProviderFailureKind::AllocationLost,
            ProviderFailureKind::AllocationOutcomeUnknown,
        ] {
            assert!(provider_fallback_allowed(kind, false), "{}", kind.as_str());
            assert!(!provider_fallback_allowed(kind, true), "{}", kind.as_str());
        }
        for kind in [
            ProviderFailureKind::Authentication,
            ProviderFailureKind::Contract,
            ProviderFailureKind::Protocol,
            ProviderFailureKind::RequestTooLarge,
            ProviderFailureKind::NotReady,
            ProviderFailureKind::PartialOutput,
            ProviderFailureKind::ClientDisconnected,
            ProviderFailureKind::Cancelled,
            ProviderFailureKind::Internal,
        ] {
            assert!(!provider_fallback_allowed(kind, false), "{}", kind.as_str());
            assert!(!provider_fallback_allowed(kind, true), "{}", kind.as_str());
        }
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
            &input,
            &channel,
            Arc::new(RunCancellation::default()),
            None,
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
            &input,
            &channel,
            Arc::new(RunCancellation::default()),
            None,
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
            id: "conversation-fallback".to_string(),
            title: None,
            task_mode: "conversation".to_string(),
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        };
        state
            .connection
            .lock()
            .expect("database lock")
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES (?1, NULL, 'conversation', 'now', 'now')",
                params![conversation.id],
            )
            .expect("conversation inserts");
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
        state
            .connection
            .lock()
            .expect("database lock")
            .execute(
                "INSERT INTO conversations(id, task_mode, created_at, updated_at)
                 VALUES('conversation-partial', 'conversation', '1', '1')",
                [],
            )
            .expect("conversation inserts");
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
            conversation_id: "conversation-partial".to_string(),
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
                 WHERE conversation_id='conversation-partial' AND role='assistant'",
                [],
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
    fn database_backup_is_reopenable_and_preserves_data() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source_path = directory.path().join("source.sqlite3");
        let backup_path = directory.path().join("backup.sqlite3");
        let source = Connection::open(&source_path).expect("source opens");
        initialize_database(&source).expect("source initializes");
        source
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('backup-conversation', NULL, 'conversation', 'now', 'now')",
                [],
            )
            .expect("conversation inserts");
        backup_connection_to(&source, &backup_path).expect("backup succeeds");
        let backup = Connection::open(backup_path).expect("backup reopens");
        let count: i64 = backup
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id = 'backup-conversation'",
                [],
                |row| row.get(0),
            )
            .expect("backup data loads");
        assert_eq!(count, 1);
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
    fn runtime_errors_are_redacted_and_bounded() {
        env::set_var("SAAA_TEST_API_KEY", "super-secret-test-value");
        let redacted = redact_runtime_text(&format!(
            "token=super-secret-test-value {}",
            "x".repeat(4_000)
        ));
        env::remove_var("SAAA_TEST_API_KEY");
        assert!(!redacted.contains("super-secret-test-value"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(redacted.chars().count() <= 2_000);
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

    fn default_settings_input() -> Vec<SaveSettingsDocumentInput> {
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

    #[test]
    fn voice_settings_require_the_fixed_gnosis_asr_contract() {
        let mut documents = default_settings_input();
        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["sttProviderId"] = json!("local-whisper");
        assert_eq!(
            validate_settings_document(voice).expect_err("local Whisper is rejected"),
            "Invalid local voice settings"
        );

        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["sttProviderId"] = json!(voice::gnosis_asr::PROVIDER_ID);
        voice.value_json["sttModel"] = json!(voice::gnosis_asr::MODEL_ID);
        validate_settings_document(voice).expect("gnosis ASR contract is accepted");
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
        state
            .connection
            .lock()
            .expect("database lock")
            .execute(
                "INSERT INTO conversations(id, task_mode, created_at, updated_at)
                 VALUES('conversation-larm', 'conversation', '1', '1')",
                [],
            )
            .expect("conversation inserts");
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
            conversation_id: "conversation-larm".to_string(),
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
