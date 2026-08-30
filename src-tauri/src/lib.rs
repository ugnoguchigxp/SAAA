use rusqlite::Connection;
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    process::Child,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::Manager;

mod app_paths;
mod backup;
mod credentials;
mod diagnostics;
pub mod ipc_contract;
mod meeting;
mod memory;
mod models;
mod persistence;
mod process_guard;
mod providers;
#[cfg(feature = "quality-eval-harness")]
pub mod quality_eval;
mod redact;
mod runtime;
mod situation;
#[cfg(test)]
mod test_support;
mod util;
mod voice;
mod voice_commands;
mod voice_contracts;
mod voice_text;

use backup::backup_connection_to;

pub(crate) use models::*;
use persistence::list_messages_from_connection;
use persistence::migrate::backup_before_migration;
use persistence::schema::initialize_database;
pub(crate) use providers::session_store::{
    begin_provider_session, finish_dynamic_lan_provider_session, finish_larm_provider_session,
    finish_provider_session, persist_conversation_success,
};
pub(crate) use providers::stream::*;
pub(crate) use runtime::codex_cli::*;
pub(crate) use runtime::codex_turn::execute_codex_turn;
#[cfg(test)]
pub(crate) use runtime::codex_turn::{
    persist_codex_thread, receive_supervised_codex_result, run_codex_turn_process,
    run_codex_turn_process_with_policy,
};
pub(crate) use runtime::run_support::*;
pub(crate) use runtime::turn_types::*;
#[cfg(test)]
pub(crate) use runtime::turns::prepare_runtime_run;
pub(crate) use runtime::turns::{execute_turn, finish_runtime_run, send_runtime_terminal_event};
pub(crate) use situation::spawn_situation_monitor;
pub(crate) use util::{database_error, new_id, now_iso, validate_identifier};
use voice::streaming_asr::{
    append_voice_asr_audio, commit_voice_asr_utterance, start_voice_asr_session,
    stop_voice_asr_session, AsrSessionManager,
};
use voice_commands::{
    delete_voice_enrollment_sample, delete_voice_profile, get_voice_profile_snapshot,
    list_tts_capabilities, read_voice_enrollment_sample, resolve_network_asr,
    save_voice_enrollment_sample, set_target_speaker_filter_enabled, speak_text,
    stage_audio_upload, stop_tts, transcribe_audio, transcribe_audio_chunk,
};
pub(crate) use voice_contracts::*;

pub(crate) use redact::{bounded_text, redact_runtime_text};

use ipc_contract::{ConversationMessage, RuntimeEvent};

const WINDOW_SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const DYNAMIC_LAN_PROVIDER_ID: &str = "lan-llm-dynamic";
const DEFAULT_DYNAMIC_LAN_HOST: &str = "localhost";
const DEFAULT_AGENT_NAME: &str = "SAAA";
const DEFAULT_USER_NAME: &str = "";
const PRIMARY_CONVERSATION_ID: &str = "conversation_primary";
const PRIMARY_CONVERSATION_TITLE: &str = "SAAAとの会話";
const CODEX_READ_ONLY_SYSTEM_CONTEXT: &str = include_str!("../../.s11tnext/codex-read-only.txt");

struct AppState {
    connection: Arc<Mutex<Connection>>,
    data_directory: PathBuf,
    context_still_recall: memory::context_still_recall::ContextStillRecallClient,
    active_runs: Mutex<HashMap<String, Arc<RunCancellation>>>,
    provider_probes: Mutex<HashMap<String, ProviderProbeStatus>>,
    interaction_policy: Mutex<()>,
    shutdown_started: AtomicBool,
    larm_gate: providers::larm::LarmRuntimeGate,
    network_asr: voice::network_asr::NetworkAsrRuntime,
    audio_uploads: voice::audio_upload::AudioUploadStore,
    tts_process: Mutex<Option<ActiveTts>>,
    streaming_tts: voice::streaming_tts::runtime::StreamingSpeechRuntime,
    situation: Arc<situation::SituationRuntime>,
    meeting: Arc<meeting::MeetingRuntime>,
    voice_profile: Arc<voice::profile::VoiceProfileRuntime>,
    voice_asr: AsrSessionManager,
}

#[derive(Clone)]
struct ProviderProbeStatus {
    ok: bool,
    checked_at: String,
    configuration_fingerprint: String,
    prior_session_rowid: i64,
}

struct ActiveTts {
    run_id: String,
    child: Child,
    artifact: Option<PathBuf>,
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

#[tauri::command]
fn frontend_ready(state: tauri::State<'_, AppState>) -> Result<(), String> {
    app_paths::frontend_ready(&state)
}

#[tauri::command]
fn get_app_snapshot(state: tauri::State<'_, AppState>) -> Result<AppSnapshot, String> {
    persistence::app_commands::get_app_snapshot(&state)
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

#[tauri::command]
fn export_diagnostics(state: tauri::State<'_, AppState>) -> Result<LocalArtifactResult, String> {
    diagnostics::export_diagnostics(&state)
}

#[tauri::command]
async fn test_model_provider(
    state: tauri::State<'_, AppState>,
    input: TestProviderInput,
) -> Result<ProviderTestResult, String> {
    providers::probe::test_model_provider(&state, input).await
}

#[tauri::command]
async fn resolve_service_harness(
    address: String,
) -> Result<providers::service_harness::HarnessResolution, String> {
    providers::service_harness::resolve_with_legacy_llm(&address).await
}

#[tauri::command]
fn set_provider_api_key(
    state: tauri::State<'_, AppState>,
    input: credentials::SetProviderApiKeyInput,
) -> Result<credentials::ProviderCredentialState, String> {
    credentials::set_api_key(&state.connection, input)
}

#[tauri::command]
fn delete_provider_api_key(
    provider_id: String,
) -> Result<credentials::ProviderCredentialState, String> {
    credentials::delete_api_key(provider_id)
}

#[tauri::command]
fn get_provider_credential_state(
    provider_id: String,
) -> Result<credentials::ProviderCredentialState, String> {
    credentials::credential_state(provider_id)
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
    let mut streaming_speech = input.presentation_mode == "visual-and-spoken";
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(&state, &input.run_id, cancellation.clone())?;
    if streaming_speech {
        if let Err(error) = state
            .streaming_tts
            .begin(&state, &input.run_id, on_event.clone())
            .await
        {
            streaming_speech = false;
            let _ = on_event.send(RuntimeEvent::SpeechFailed {
                run_id: input.run_id.clone(),
                message: redact_runtime_text(&error),
                recovery: "Check the speech provider and try another response.".to_string(),
            });
        }
    }
    let event_sink = state.streaming_tts.event_sink(on_event, streaming_speech);
    let result = execute_turn(&state, &input, &event_sink, cancellation.clone(), None).await;
    if result.is_err() && streaming_speech {
        state.streaming_tts.cancel(&input.run_id);
    }
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
    state.streaming_tts.cancel(&run_id);
    Ok(())
}

#[tauri::command]
async fn meeting_preflight(
    state: tauri::State<'_, AppState>,
    input: meeting::PreflightInput,
) -> Result<meeting::PreflightResult, String> {
    let asr_health = voice::session::probe_selected_asr(&state).await;
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
    meeting::commands::start_meeting_inner(&state, &input)
}

#[tauri::command]
fn pause_meeting(
    state: tauri::State<'_, AppState>,
    input: meeting::SessionInput,
) -> Result<meeting::MeetingSnapshot, String> {
    meeting::commands::pause_meeting(&state, input)
}

#[tauri::command]
fn resume_meeting(
    state: tauri::State<'_, AppState>,
    input: meeting::SessionInput,
) -> Result<meeting::MeetingSnapshot, String> {
    meeting::commands::resume_meeting(&state, input)
}

#[tauri::command]
fn stop_meeting(
    state: tauri::State<'_, AppState>,
    input: meeting::SessionInput,
) -> Result<meeting::MeetingSnapshot, String> {
    meeting::commands::stop_meeting(&state, input)
}

#[tauri::command]
async fn append_meeting_audio_segment(
    state: tauri::State<'_, AppState>,
    input: meeting::SegmentInput,
) -> Result<meeting::SegmentResult, String> {
    meeting::commands::append_meeting_audio_segment(&state, input).await
}

#[tauri::command]
async fn preview_meeting_audio_segment(
    state: tauri::State<'_, AppState>,
    input: meeting::PreviewSegmentInput,
) -> Result<(), String> {
    meeting::commands::preview_meeting_audio_segment(&state, input).await
}

#[tauri::command]
fn save_meeting_transcript(
    state: tauri::State<'_, AppState>,
    input: meeting::SessionInput,
) -> Result<meeting::MeetingSnapshot, String> {
    meeting::commands::save_meeting_transcript(&state, input)
}

#[tauri::command]
fn discard_meeting(
    state: tauri::State<'_, AppState>,
    input: meeting::SessionInput,
) -> Result<(), String> {
    meeting::commands::discard_meeting(&state, input)
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

#[tauri::command]
fn save_settings_documents(
    state: tauri::State<'_, AppState>,
    input: SaveSettingsDocumentsInput,
) -> Result<Vec<SettingsDocument>, String> {
    persistence::app_commands::save_settings_documents(&state, input)
}

#[tauri::command]
fn create_conversation(
    state: tauri::State<'_, AppState>,
    input: CreateConversationInput,
) -> Result<Conversation, String> {
    persistence::app_commands::create_conversation(&state, input)
}

#[tauri::command]
fn append_message(
    state: tauri::State<'_, AppState>,
    input: AppendMessageInput,
) -> Result<ConversationMessage, String> {
    persistence::app_commands::append_message(&state, input)
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

fn shutdown_app_state(state: &AppState) {
    state.voice_asr.shutdown();
    state.streaming_tts.shutdown();
    if let Ok(mut process) = state.tts_process.lock() {
        if let Some(mut active) = process.take() {
            let _ = active.child.kill();
            let _ = active.child.wait();
            if let Some(path) = active.artifact {
                let _ = fs::remove_file(path);
            }
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
        .plugin(tauri_plugin_llm_fetch::init())
        .setup(|app| {
            let database_path = app_paths::application_database_path(app)?;
            let voice_resource_directory = app
                .path()
                .resolve("voice", tauri::path::BaseDirectory::Resource)?;
            let voice_data_directory = database_path
                .parent()
                .ok_or_else(|| std::io::Error::other("Database path has no parent directory"))?
                .to_path_buf();
            voice::cloud_tts::cleanup_cache(&voice_data_directory.join("tts-cache"))
                .map_err(std::io::Error::other)?;
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
            let bundled_web_fetch = app.path().resolve(
                if cfg!(windows) {
                    "bin/webfetch.exe"
                } else {
                    "bin/webfetch"
                },
                tauri::path::BaseDirectory::Resource,
            )?;
            if bundled_web_fetch.is_file() {
                let _ = runtime::web_fetch::BUNDLED_WEB_FETCH_PATH.set(bundled_web_fetch);
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
                provider_probes: Mutex::new(HashMap::new()),
                interaction_policy: Mutex::new(()),
                shutdown_started: AtomicBool::new(false),
                larm_gate: providers::larm::LarmRuntimeGate::initialize(),
                network_asr: voice::network_asr::NetworkAsrRuntime::new()
                    .map_err(std::io::Error::other)?,
                audio_uploads: voice::audio_upload::AudioUploadStore::default(),
                tts_process: Mutex::new(None),
                streaming_tts: voice::streaming_tts::runtime::StreamingSpeechRuntime::default(),
                situation,
                meeting: Arc::new(meeting::MeetingRuntime::new()),
                voice_profile,
                voice_asr: AsrSessionManager::default(),
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
            stage_audio_upload,
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
            resolve_service_harness,
            set_provider_api_key,
            delete_provider_api_key,
            get_provider_credential_state,
            resolve_network_asr,
            transcribe_audio_chunk,
            transcribe_audio,
            start_voice_asr_session,
            append_voice_asr_audio,
            commit_voice_asr_utterance,
            stop_voice_asr_session,
            speak_text,
            list_tts_capabilities,
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
    use crate::persistence::save_settings_documents_to_connection;
    use crate::test_support::*;
    use rusqlite::params;
    use serde_json::{json, Value};
    use std::{
        env,
        io::Read,
        process::{Command, Stdio},
        sync::mpsc,
        thread,
    };

    fn begin_test_provider_session(
        state: &AppState,
        runtime_run_id: &str,
        provider_id: &str,
        provider_kind: &str,
    ) -> Result<String, String> {
        let fingerprint = {
            let connection = state
                .connection
                .lock()
                .map_err(|_| "Database lock unavailable".to_string())?;
            crate::persistence::effective_route::load_conversation_configuration_fingerprint(
                &connection,
            )?
        };
        begin_provider_session(
            state,
            runtime_run_id,
            provider_id,
            provider_kind,
            &fingerprint,
        )
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

        let session_id = begin_test_provider_session(
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
    fn dynamic_lan_session_persists_release_success_and_deferred_cleanup() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        connection
            .execute(
                "INSERT INTO conversations(id, task_mode, created_at, updated_at)
                 VALUES('conversation-dynamic_lan-cleanup', 'conversation', '1', '1')",
                [],
            )
            .expect("conversation inserts");
        let state = app_state(connection);

        for (run_id, cleanup, expected_status, expected_kind) in [
            (
                "run-dynamic_lan-released",
                CleanupOutcome::Released,
                "released",
                None,
            ),
            (
                "run-dynamic_lan-deferred",
                CleanupOutcome::DynamicLanDeferredToTtl { kind: "network" },
                "deferred-to-ttl",
                Some("network"),
            ),
        ] {
            begin_simple_runtime_run(
                &state,
                run_id,
                "conversation-dynamic_lan-cleanup",
                "conversation.respond",
                "lan-llm-dynamic",
            )
            .expect("runtime starts");
            let session_id =
                begin_test_provider_session(&state, run_id, "lan-llm-dynamic", "openai-compatible")
                    .expect("provider session starts");
            finish_dynamic_lan_provider_session(&state, &session_id, "completed", None, cleanup)
                .expect("dynamic_lan session finalizes");
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
    fn dynamic_lan_cleanup_keeps_release_debt_across_connection_replacement() {
        let deferred = CleanupOutcome::DynamicLanDeferredToTtl { kind: "network" };
        assert_eq!(
            merge_dynamic_lan_cleanup(deferred, CleanupOutcome::Released),
            deferred
        );
        assert_eq!(
            merge_dynamic_lan_cleanup(CleanupOutcome::Released, CleanupOutcome::Released),
            CleanupOutcome::Released
        );
        assert_eq!(
            dynamic_lan_cleanup_from_release_failure(Some(
                providers::dynamic_lan::ErrorKind::Timeout
            )),
            CleanupOutcome::DynamicLanDeferredToTtl { kind: "timeout" }
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
                    translation_enabled: false,
                },
                Ok(()),
            )
            .expect("meeting preflight succeeds");

        let error = meeting::commands::start_meeting_inner(
            &state,
            &meeting::StartInput {
                session_id: "meeting-blocked-by-agent".to_string(),
                microphone_device_id: "default".to_string(),
                microphone_enabled: true,
                system_audio_enabled: false,
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        let coding = StartTurnInput {
            run_id: "run-coding-no-workspace".to_string(),
            conversation_id: "workspace-required".to_string(),
            content: "coding request".to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
        let resolved = app_paths::validate_readiness_data_directory(directory.path(), &normal)
            .expect("isolated directory is accepted");
        assert_eq!(
            resolved,
            directory.path().canonicalize().expect("path resolves")
        );
        assert!(
            app_paths::validate_readiness_data_directory(directory.path(), directory.path())
                .expect_err("normal app data is rejected")
                .contains("normal application data")
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
                .expect("permissions change");
            assert!(
                app_paths::validate_readiness_data_directory(directory.path(), &normal)
                    .expect_err("broad permissions are rejected")
                    .contains("mode 0700")
            );
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
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
                max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
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
        let request = request_body.lock().expect("request lock").clone();
        assert!(request.contains("\"max_tokens\":2048"));
        assert!(request.contains("\"enable_thinking\":false"));
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
    async fn dynamic_lan_stream_policy_rejects_a_non_sse_completion() {
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
            ..direct_provider("dynamic_lan-sse-policy", "local")
        };
        let input = StartTurnInput {
            run_id: "run-dynamic_lan-sse-policy".to_string(),
            conversation_id: "conversation-dynamic_lan-sse-policy".to_string(),
            content: "test".to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        let history = vec![ConversationMessage {
            id: "message-dynamic_lan-sse-policy".to_string(),
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
                max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
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
                        "data: {}\n\n",
                        "data: [DONE]\n\n"
                    ),
                    first_delta,
                    second_delta,
                    json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]})
                ),
                concat!(
                    "HTTP/1.1 200 OK\r\n",
                    "Content-Type: text/event-stream\r\n",
                    "Connection: close\r\n\r\n",
                    "data: {\"choices\":[{\"delta\":{\"content\":\"履歴を確認しました\"}}]}\n\n",
                    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        prepare_runtime_run(&state, &input).expect("runtime prepares");
        let session_id = begin_test_provider_session(
            &state,
            &input.run_id,
            "recall-fixture",
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
            ..direct_provider("recall-fixture", "local")
        };
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
        let outcome = stream_model_provider(
            &provider,
            &history,
            5_000,
            ModelStreamContext {
                reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
                max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
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
        assert_eq!(first["tools"].as_array().expect("tools array").len(), 3);
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
                "recall_skill",
                "web_search",
                "fetch_content"
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                "data: [DONE]\n\n"
            );
            let final_response = concat!(
                "HTTP/1.1 200 OK\r\n",
                "Content-Type: text/event-stream\r\n",
                "Connection: close\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"too late\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        prepare_runtime_run(&state, &input).expect("runtime prepares");
        let session_id = begin_test_provider_session(
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
                max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
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
                        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        let channel: tauri::ipc::Channel<RuntimeEvent> =
            tauri::ipc::Channel::new(|_| Err(tauri::Error::Io(std::io::Error::other("closed"))));
        let outcome = stream_model_provider(
            &provider,
            &[],
            2_000,
            ModelStreamContext {
                reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
                max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
        let outcome = stream_model_provider(
            &provider,
            &[],
            2_000,
            ModelStreamContext {
                reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
                max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
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
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
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
            .value_json = json!({
            "harness": { "address": "http://localhost:9810" }, "providers": [{
            "kind": "openai-compatible", "id": "primary", "enabled": true, "label": "Primary", "location": "local",
            "endpoint": format!("http://{primary_address}/v1"), "model": "primary-model", "authentication": "none"
        }, {
            "kind": "openai-compatible", "id": "fallback", "enabled": true, "label": "Fallback", "location": "local",
            "endpoint": format!("http://{fallback_address}/v1"), "model": "fallback-model", "authentication": "none"
        }], "reasoningEffort": "medium"});
        let route = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("route settings");
        route.value_json["conversationRespond"]["source"] = json!("provider");
        route.value_json["conversationRespond"]["primaryProviderId"] = json!("primary");
        route.value_json["conversationRespond"]["fallbackProviderIds"] = json!(["fallback"]);
        route.value_json["voiceSpeak"]["source"] = json!("harness");
        route.value_json["voiceSpeak"]["providerId"] = Value::Null;
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
            .value_json = json!({
            "harness": { "address": "http://localhost:9810" }, "providers": [{
            "kind": "openai-compatible", "id": "partial-primary", "enabled": true, "label": "Partial primary", "location": "local",
            "endpoint": format!("http://{primary_address}/v1"), "model": "primary-model", "authentication": "none"
        }, {
            "kind": "openai-compatible", "id": "forbidden-fallback", "enabled": true, "label": "Forbidden fallback", "location": "local",
            "endpoint": format!("http://{fallback_address}/v1"), "model": "fallback-model", "authentication": "none"
        }], "reasoningEffort": "medium"});
        let route = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        route.value_json["conversationRespond"]["source"] = json!("provider");
        route.value_json["conversationRespond"]["primaryProviderId"] = json!("partial-primary");
        route.value_json["conversationRespond"]["fallbackProviderIds"] =
            json!(["forbidden-fallback"]);
        route.value_json["voiceSpeak"]["source"] = json!("harness");
        route.value_json["voiceSpeak"]["providerId"] = Value::Null;
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
        let output = voice::network_asr::resample_pcm(&input, 48_000, 16_000);
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
                    retry_input_message_id: None,
                    input_origin: "text".to_string(),
                    presentation_mode: "visual".to_string(),
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
            "\"runtime\":\"runtime_turn\",\"node\":\"dynamic_lan\",\"status\":\"HOT\",",
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
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let tool_stream_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nX-Request-ID: req_tool\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{tool_stream_body}",
            tool_stream_body.len()
        );
        let stream_body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"LARM ok\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
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
            .value_json = json!({
            "harness": { "address": "http://localhost:9810" }, "providers": [{
                "kind": "larm", "id": "larm-primary", "enabled": true, "label": "LARM",
                "location": "local", "baseUrl": format!("http://{address}"),
                "tokenEnv": "LARM_API_TOKEN", "allocationTtlSeconds": 300,
                "allocationStartupTimeoutSeconds": 5, "allowFallbackByDefault": false,
                "deploymentPolicy": "existing-only"
            }], "reasoningEffort": "medium" });
        let routing = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        routing.value_json["conversationRespond"]["source"] = json!("provider");
        routing.value_json["conversationRespond"]["primaryProviderId"] = json!("larm-primary");
        routing.value_json["conversationRespond"]["fallbackProviderIds"] = json!([]);
        routing.value_json["voiceSpeak"]["source"] = json!("harness");
        routing.value_json["voiceSpeak"]["providerId"] = Value::Null;
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
            retry_input_message_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
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
