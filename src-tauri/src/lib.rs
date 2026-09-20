mod app_state;
use app_state::{AppState, ProviderProbeStatus, RunCancellation};
#[path = "providers/larm_voice/mod.rs"]
mod larm_voice;
#[cfg(test)]
use rusqlite::Connection;
#[cfg(test)]
use std::fs;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::Manager;

mod adaptive_improvement;
mod app_paths;
mod backup;
mod coding;
#[path = "runtime/command_registry.rs"]
mod command_registry;
mod credentials;
mod database_backup;
mod diagnostics;
pub mod generated_capabilities;
mod generative_ui;
pub mod ipc_contract;
mod memory;
mod models;
mod persistence;
mod process_guard;
mod providers;
#[cfg(feature = "quality-eval-harness")]
pub mod quality_eval;
mod redact;
mod role_routing;
mod runtime;
mod schedule;
mod situation;
mod steward;
#[cfg(test)]
mod test_state;
#[cfg(test)]
mod test_support;
pub mod tool_selection;
mod util;
mod voice;
pub mod voice_asr_contract;
mod voice_behavior;
mod voice_commands;
mod voice_text;
#[cfg(test)]
mod wasm_host_poc;

pub(crate) use models::*;
#[cfg(test)]
use persistence::conversations::list_messages_from_connection;
#[cfg(test)]
use persistence::schema::initialize_database;
use persistence::{list_message_page_from_connection, SqliteReaders, SqliteWriter};
pub(crate) use providers::session_store::{
    begin_provider_session, finish_dynamic_lan_provider_session, finish_provider_session,
    persist_conversation_success, persist_conversation_success_with_state,
};
pub(crate) use providers::stream::*;
pub(crate) use redact::{bounded_text, redact_runtime_text};
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
pub(crate) use runtime::turns::finish_runtime_run;
#[cfg(test)]
pub(crate) use runtime::turns::prepare_runtime_run;
pub(crate) use runtime::turns::{execute_turn, send_runtime_terminal_event};
pub(crate) use situation::spawn_situation_monitor;
pub(crate) use util::{database_error, new_id, now_iso, validate_identifier};
use voice::streaming_asr::{
    append_voice_asr_audio, commit_voice_asr_utterance, start_voice_asr_session,
    stop_voice_asr_session, AsrSessionManager,
};
use voice_commands::{
    delete_voice_enrollment_sample, delete_voice_profile, get_voice_profile_snapshot,
    read_voice_enrollment_sample, save_voice_enrollment_sample, set_target_speaker_filter_enabled,
    stage_audio_upload, stop_tts,
};

#[cfg(test)]
use ipc_contract::ConversationMessage;
use ipc_contract::ConversationMessagePage;
#[cfg(test)]
use ipc_contract::RuntimeEvent;
use voice_behavior::{
    get_conversation_voice_policy, reset_conversation_voice_policy,
    update_conversation_voice_policy,
};

const WINDOW_SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const DYNAMIC_LAN_PROVIDER_ID: &str = "lan-llm-dynamic";
const DEFAULT_DYNAMIC_LAN_HOST: &str = "localhost";
const DEFAULT_AGENT_NAME: &str = "SAAA";
const DEFAULT_USER_NAME: &str = "";
const PRIMARY_CONVERSATION_ID: &str = "conversation_primary";
const PRIMARY_CONVERSATION_TITLE: &str = "SAAAとの会話";
const CODEX_READ_ONLY_SYSTEM_CONTEXT: &str = include_str!("../../.s11tnext/codex-read-only.txt");

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
    state.situation.snapshot_locked(&state.sqlite_readers)
}

#[tauri::command]
fn set_situation_monitoring(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<situation::contracts::SituationSnapshot, String> {
    state
        .situation
        .set_monitoring(&state.sqlite_writer, enabled)?;
    if enabled {
        spawn_situation_monitor(state.sqlite_writer.clone(), state.situation.clone());
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
    state
        .sqlite_writer
        .write(|connection| situation::repository::submit_feedback(connection, &input))?;
    state.situation.snapshot_locked(&state.sqlite_readers)
}

#[tauri::command]
fn clear_situation_history(
    state: tauri::State<'_, AppState>,
) -> Result<situation::contracts::SituationSnapshot, String> {
    state.situation.clear_history(&state.sqlite_writer)?;
    state.situation.snapshot_locked(&state.sqlite_readers)
}

#[tauri::command]
fn get_situation_review_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<situation::contracts::SituationReviewSnapshot, String> {
    state.sqlite_readers.read(|connection| {
        Ok(situation::contracts::SituationReviewSnapshot {
            active_profile: situation::calibration::active_profile(connection)?,
            quality: situation::repository::quality_metrics(connection)?,
            feedback_queue: situation::repository::feedback_queue(connection)?,
            latest_run: situation::calibration::latest_run(connection)?,
            candidates: situation::calibration::candidates(connection)?,
        })
    })
}

#[tauri::command]
fn create_situation_calibration_candidate(
    state: tauri::State<'_, AppState>,
    parameters: situation::contracts::CalibrationParameters,
) -> Result<situation::calibration::CalibrationProfile, String> {
    state
        .sqlite_writer
        .write(|connection| situation::calibration::create_candidate(connection, parameters))
}

#[tauri::command]
async fn run_situation_calibration(
    state: tauri::State<'_, AppState>,
    profile_id: String,
) -> Result<situation::calibration::CalibrationRun, String> {
    validate_identifier(&profile_id, "calibration profile id")?;
    let readers = state.sqlite_readers.clone();
    let writer = state.sqlite_writer.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let profile = readers
            .read(|connection| situation::calibration::profile_by_id(connection, &profile_id))?;
        if profile.status != "candidate" {
            return Err("Only candidate profiles can be replayed".to_string());
        }
        let metrics = situation::calibration::replay_metrics(&profile)?;
        writer.write(|connection| {
            situation::calibration::save_run(
                connection,
                &profile_id,
                "completed",
                Some(metrics),
                None,
            )
        })
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
    state.situation.decide_calibration(
        &state.sqlite_writer,
        &profile_id,
        &decision,
        &reason_code,
    )?;
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
    credentials::set_api_key(&state.sqlite_readers, input)
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
fn record_frontend_audit_event(
    state: tauri::State<'_, AppState>,
    input: persistence::audit::FrontendAuditEventInput,
) -> Result<(), String> {
    persistence::audit::record_frontend_event(&state, &input)
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
fn save_settings_documents(
    state: tauri::State<'_, AppState>,
    input: SaveSettingsDocumentsInput,
) -> Result<Vec<SettingsDocument>, String> {
    persistence::app_commands::save_settings_documents(&state, input)
}

#[tauri::command]
fn set_voice_listening_enabled(
    state: tauri::State<'_, AppState>,
    input: SetVoiceListeningEnabledInput,
) -> Result<SettingsDocument, String> {
    persistence::settings::set_voice_listening_enabled(&state, input.enabled)
}

#[tauri::command]
async fn list_messages(
    state: tauri::State<'_, AppState>,
    input: ListMessagesInput,
) -> Result<ConversationMessagePage, String> {
    const MESSAGE_PAGE_SIZE: u64 = 10;
    validate_identifier(&input.conversation_id, "conversation id")?;
    let readers = state.sqlite_readers.clone();
    let conversation_id = input.conversation_id;
    let cursor = input.cursor;
    readers
        .read_async(move |connection| {
            list_message_page_from_connection(
                connection,
                &conversation_id,
                cursor.as_deref(),
                MESSAGE_PAGE_SIZE,
            )
        })
        .await
}

/// Builds the generated-capability service from the trusted runtime configuration
/// (`SAAA_LLANG_RUNTIME_CONFIG`). When it is unset or invalid the feature stays disabled and
/// SAAA starts normally.
fn build_capability_service(
    sqlite_writer: Arc<SqliteWriter>,
    data_directory: &std::path::Path,
) -> generated_capabilities::service::CapabilityService {
    let config = std::env::var_os("SAAA_LLANG_RUNTIME_CONFIG")
        .map(PathBuf::from)
        .and_then(|path| {
            generated_capabilities::host::runtime_bundle::load_config(&path)
                .map_err(|error| {
                    eprintln!("generated capability runtime config rejected: {error}");
                })
                .ok()
        });
    let ledger_directory = data_directory
        .join("generated-capabilities")
        .join("acceptance");
    generated_capabilities::service::CapabilityService::build(
        sqlite_writer,
        data_directory,
        ledger_directory,
        config,
    )
}

pub(crate) fn open_database_writer(
    database_path: &std::path::Path,
) -> Result<SqliteWriter, String> {
    SqliteWriter::open(database_path).map_err(|error| error.to_string())
}

#[tauri::command]

fn shutdown_app_state(state: &AppState) {
    memory::personal_state::worker::interrupt();
    coding::commands::shutdown(state);
    state.generated_capabilities.shutdown();
    // Stop admitting new external MCP calls and close sessions. The task is spawned because the
    // close path is synchronous; in-flight calls keep their indeterminate outcome.
    if let Some(manager) = state.tool_selection.mcp_manager() {
        tauri::async_runtime::spawn(async move { manager.shutdown().await });
    }
    // Stop the published MCP listener; the D4 manager and result writer stay alive for their tasks.
    tool_selection::mcp_server::shutdown_slot(&state.mcp_server);
    state.voice_asr.shutdown();
    state.streaming_tts.shutdown();
    if let Ok(active_runs) = state.active_runs.lock() {
        for cancellation in active_runs.values() {
            cancellation.cancel();
        }
    }
    let _ = state.situation.flush_quality(&state.sqlite_writer);
    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::Inactive);
    state
        .situation
        .set_audio_state(situation::contracts::AudioState::Silent);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Snapshot the opt-in configuration once; failures are reported on use.
    let _ = providers::reasoning_mcp::configured("voice");
    let _ = larm_voice::enabled();
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
            let sqlite_writer = Arc::new(SqliteWriter::open(&database_path)?);
            let sqlite_readers =
                SqliteReaders::open(&database_path).map_err(std::io::Error::other)?;
            sqlite_writer
                .write(|connection| voice_profile.reconcile_readiness(connection))
                .map_err(std::io::Error::other)?;
            sqlite_writer
                .write(|connection| {
                    voice::profile::reconcile_voice_profile_storage(
                        connection,
                        &voice_data_directory,
                    )
                })
                .map_err(std::io::Error::other)?;
            let (situation_settings, latest_situation, active_profile) = sqlite_readers
                .read(|connection| {
                    Ok((
                        situation::repository::load_settings(connection)?,
                        situation::repository::latest_entry(connection)?,
                        situation::calibration::active_profile(connection)?,
                    ))
                })
                .map_err(std::io::Error::other)?;
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
                spawn_situation_monitor(sqlite_writer.clone(), situation.clone());
            }
            memory::personal_state::worker::spawn(Arc::downgrade(&sqlite_writer));
            let generated_capabilities = Arc::new(build_capability_service(
                sqlite_writer.clone(),
                &voice_data_directory,
            ));
            let (requests, generator, packager) =
                generated_capabilities::generation::packager::production_runtime(
                    sqlite_writer.clone(),
                    &voice_data_directory,
                );
            let generation = Some(Arc::new(
                generated_capabilities::generation::service::GenerationService::new(
                    sqlite_writer.clone(),
                    generated_capabilities.clone(),
                    requests,
                    generator,
                    packager,
                    &voice_data_directory,
                ),
            ));
            if let Some(generation) = generation.as_ref() {
                match generation.reconcile() {
                    Ok(summary)
                        if summary.interrupted_jobs > 0
                            || !summary.orphan_inspections.is_empty() =>
                    {
                        eprintln!("generated capability generation recovery applied: {summary:?}");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("generated capability generation recovery skipped: {error}")
                    }
                }
            }
            if generated_capabilities.is_ready() {
                match generated_capabilities::recovery::reconcile_startup(&generated_capabilities) {
                    Ok(summary) => {
                        if summary.interrupted_checks
                            + summary.interrupted_calls
                            + summary.interrupted_imports
                            + summary.missing_packages.len()
                            + summary.inconsistent_capabilities.len()
                            + summary.orphan_packages.len()
                            > 0
                        {
                            eprintln!("generated capability recovery applied: {summary:?}");
                        }
                    }
                    Err(error) => {
                        eprintln!("generated capability recovery skipped: {error}");
                    }
                }
            }
            // Publication is fixed at startup; changing it requires a restart. A rejected file
            // disables only the generated conversation tools.
            let generated_tools =
                generated_capabilities::publication::GeneratedToolsConfig::from_environment();
            if let Some(diagnostic) = generated_tools.diagnostic {
                eprintln!("generated tools config disabled: {diagnostic}");
            }
            // Tool selection is fixed at startup as well. An unset configuration keeps the
            // legacy direct mode; discovery builds the local worker only from local files.
            let tool_selection_config = tool_selection::ToolSelectionConfig::from_environment();
            if let Some(diagnostic) = tool_selection_config.diagnostic {
                eprintln!("tool selection config disabled: {diagnostic}");
            }
            let tool_selection = Arc::new(tool_selection::build_service(
                sqlite_writer.clone(),
                &tool_selection_config,
                Some(generated_capabilities.clone()),
            ));
            // D5: publish the three entry points, learning our own endpoint before the D4 poll loop
            // so a self-referencing source is already refused.
            let mcp_server = tauri::async_runtime::block_on(
                tool_selection::mcp_server::start_from_environment(
                    tool_selection.clone(),
                    sqlite_writer.clone(),
                ),
            );
            // The external MCP poll loop is opt-in: it only exists when a sources file is
            // configured. It performs an immediate sync before serving.
            if let Some(manager) = tool_selection.mcp_manager() {
                manager.start_background();
            }
            app.manage(AppState {
                sqlite_writer,
                sqlite_readers,
                data_directory: voice_data_directory,
                context_still_recall:
                    memory::context_still_recall::ContextStillRecallClient::from_environment(),
                active_runs: Mutex::new(HashMap::new()),
                provider_probes: Mutex::new(HashMap::new()),
                interaction_policy: Mutex::new(()),
                shutdown_started: AtomicBool::new(false),
                audio_uploads: voice::audio_upload::AudioUploadStore::default(),
                streaming_tts: voice::streaming_tts::runtime::StreamingSpeechRuntime::default(),
                voice_behavior: voice_behavior::VoiceBehaviorRuntime::default(),
                situation,
                voice_profile,
                voice_asr: AsrSessionManager::default(),
                generated_capabilities,
                generation,
                generated_tools,
                tool_selection,
                mcp_server: Mutex::new(mcp_server),
                schedule: Arc::new(schedule::Handle::default()),
            });
            let recovery_now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            app.state::<AppState>()
                .sqlite_writer
                .write(|connection| {
                    role_routing::recovery::reconcile_startup(connection, recovery_now_ms)
                        .map(|_| ())
                })
                .map_err(|error| format!("role-routing startup recovery: {error}"))?;
            adaptive_improvement::start_worker(app.state::<AppState>().sqlite_writer.clone());
            schedule::hydrate(&app.state::<AppState>());
            schedule::start_loop(app.handle().clone());
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
                let coding_active = window.state::<AppState>().sqlite_readers.read(|c| c.query_row("SELECT EXISTS(SELECT 1 FROM coding_runs WHERE state IN ('starting','running','stopping'))",[],|r|r.get::<_,bool>(0)).map_err(database_error)).unwrap_or(false);
                let grace = if coding_active { Duration::from_secs(30) } else { WINDOW_SHUTDOWN_GRACE };
                let deadline = tokio::time::Instant::now() + grace;
                loop {
                    let no_active_runs = window
                        .state::<AppState>()
                        .active_runs
                        .lock()
                        .map(|active| active.is_empty())
                        .unwrap_or(true);
                    let no_coding_runs = window.state::<AppState>().sqlite_readers.read(|c| c.query_row("SELECT NOT EXISTS(SELECT 1 FROM coding_runs WHERE state IN ('starting','running','stopping'))",[],|r|r.get::<_,bool>(0)).map_err(database_error)).unwrap_or(false);
                    if (no_active_runs && no_coding_runs) || tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                let _ = window.close();
            });
        })
        .invoke_handler(command_registry::saaa_invoke_handler!())
        .build(tauri::generate_context!())
        .expect("error while building SAAA")
        .run(|_, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                tauri::async_runtime::block_on(memory::personal_state::product_binding::shutdown());
                tauri::async_runtime::block_on(larm_voice::shutdown());
            }
        });
}

mod tests;

mod ipc_receiver_tests;

mod test_environment;

#[cfg(feature = "quality-eval-harness")]
pub use memory::personal_state::live_harness as personal_state_harness;
