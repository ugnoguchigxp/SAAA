mod app_state;
use app_state::{AppState, ProviderProbeStatus, RunCancellation};
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
mod adaptive_evaluation;
mod adaptive_improvement;
mod app_paths;
mod artifact_preview;
mod backup;
mod coding;
#[path = "runtime/command_registry.rs"]
mod command_registry;
mod credentials;
mod database_backup;
mod diagnosis;
mod diagnostics;
pub mod generated_capabilities;
mod generative_ui;
#[cfg(feature = "provider-diagnostics")]
pub mod harness_llm_diagnostic;
pub mod ipc_contract;
mod memory;
mod models;
mod persistence;
mod process_guard;
mod providers;
#[cfg(feature = "quality-eval-harness")]
pub mod quality_eval;
mod records;
mod redact;
mod role_routing;
pub mod runtime;
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
mod window_size;
pub(crate) use ipc_contract::RuntimeEvent;
use ipc_contract::{ConversationMessage, ConversationMessagePage};
pub(crate) use models::*;
use persistence::list_message_page_from_connection;
pub(crate) use persistence::schema::initialize_database;
use persistence::{SqliteReaders, SqliteWriter};
pub(crate) use providers::session_store::{
    begin_provider_session, finish_dynamic_lan_provider_session, finish_provider_session,
    persist_conversation_success, persist_conversation_success_with_state,
};
pub(crate) use providers::stream::*;
pub(crate) use redact::{bounded_text, redact_runtime_text};
pub(crate) use runtime::codex_cli::*;
pub(crate) use runtime::codex_turn::execute_codex_turn;
pub(crate) use runtime::run_support::*;
pub(crate) use runtime::turn_types::*;
pub(crate) use runtime::turns::{execute_turn, send_runtime_terminal_event};
pub(crate) use situation::spawn_situation_monitor;
pub(crate) use util::{database_error, new_id, now_iso, validate_identifier};
use voice::audio_backend::commands::{
    audio_backend_status, interrupt_native_voice_playback, start_native_voice_capture,
    stop_native_voice_capture,
};
use voice::streaming_asr::{
    append_voice_asr_audio, commit_voice_asr_utterance, start_voice_asr_session,
    stop_voice_asr_session, AsrSessionManager,
};
use voice_behavior::{
    get_conversation_voice_policy, reset_conversation_voice_policy,
    update_conversation_voice_policy,
};
use voice_commands::{
    delete_voice_enrollment_sample, delete_voice_profile, get_voice_profile_snapshot,
    read_voice_enrollment_sample, save_voice_enrollment_sample, set_target_speaker_filter_enabled,
    stage_audio_upload, stop_tts,
};
mod ipc_receiver_tests;
mod test_environment;
mod tests;
#[path = "lib/window_shutdown_grace.rs"]
pub(crate) mod window_shutdown_grace;
use window_shutdown_grace::*;
pub(crate) use window_shutdown_grace::{
    DEFAULT_AGENT_NAME, DEFAULT_DYNAMIC_LAN_HOST, DEFAULT_USER_NAME, DYNAMIC_LAN_PROVIDER_ID,
    PRIMARY_CONVERSATION_ID, PRIMARY_CONVERSATION_TITLE,
};

pub fn run_vpio_probe() -> i32 {
    voice::audio_backend::run_probe()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Snapshot the opt-in configuration once; failures are reported on use.
    let _ = providers::reasoning_mcp::configured("voice");
    // WF-01: debug-only worker presentation. Release builds ignore the env
    // switch entirely; the plugin config validation rejects visible workers
    // outside `cfg(debug_assertions)`.
    let llm_fetch_plugin = match std::env::var("SAAA_LLM_FETCH_DEBUG_WINDOW") {
        Ok(value) if value == "1" && cfg!(debug_assertions) => {
            tauri_plugin_llm_fetch::Builder::default()
                .config(tauri_plugin_llm_fetch::Config {
                    debug_worker_visible: true,
                    debug_worker_devtools: true,
                    max_characters: 20_000,
                    allow_http: false,
                    require_reliable_background: true,
                    ..Default::default()
                })
                .build()
        }
        _ => tauri_plugin_llm_fetch::init(),
    };
    let artifact_preview = artifact_preview::PreviewRuntime::default();
    let protocol_runtime = artifact_preview.clone();
    tauri::Builder::default()
        .plugin(llm_fetch_plugin)
        .register_asynchronous_uri_scheme_protocol(
            "saaa-artifact-preview",
            move |_ctx, request, responder| {
                responder.respond(artifact_preview::protocol_respond(
                    &protocol_runtime,
                    request,
                ));
            },
        )
        .setup(move |app| {
            let database_path = app_paths::application_database_path(app)?;
            let voice_resource_directory = app
                .path()
                .resolve("voice", tauri::path::BaseDirectory::Resource)?;
            let voice_data_directory = database_path
                .parent()
                .ok_or_else(|| std::io::Error::other("Database path has no parent directory"))?
                .to_path_buf();
            if let Some(window) = app.get_webview_window("main") {
                window_size::restore(&window, &voice_data_directory);
            }
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
            let bundled_role_routing_codex = app.path().resolve(
                if cfg!(windows) {
                    "bin/role-routing-codex.exe"
                } else {
                    "bin/role-routing-codex"
                },
                tauri::path::BaseDirectory::Resource,
            )?;
            if bundled_role_routing_codex.is_file() {
                let _ = role_routing::adapters::codex::BUNDLED_ROLE_ROUTING_CODEX_PATH
                    .set(bundled_role_routing_codex);
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
            // WF-04/WF-10: hand the registered plugin manager to the WebFetch
            // runtime once. Debug window presentation is opt-in via
            // `SAAA_LLM_FETCH_DEBUG_WINDOW=1` on debug builds only; the
            // plugin config itself stays hidden-by-default (WF-01).
            {
                use tauri_plugin_llm_fetch::LlmFetchExt;
                let manager = app.llm_fetch().0.clone();
                let content = std::sync::Arc::new(
                    runtime::web_fetch::content::TauriWebViewContentFetcher::new(manager),
                );
                let search = runtime::web_fetch::search::RustSearchProvider::new()
                    .map_err(|failure| std::io::Error::other(failure.safe_message))?;
                runtime::web_fetch::install_runtime(runtime::web_fetch::WebFetchRuntime {
                    content,
                    search: std::sync::Arc::new(search),
                });
            }
            let sqlite_writer = Arc::new(SqliteWriter::open(&database_path)?);
            credentials::install_database(sqlite_writer.clone());
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
                context_still_search:
                    memory::context_still_search::ContextStillSearchClient::from_environment(),
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
                steward_wake: steward::pump::Wake::default(),
                artifact_preview,
                reachability: std::sync::Arc::new(providers::reachability::ReachabilityState::default()),
                reachability_kick: std::sync::Arc::new(tokio::sync::Notify::new()),
                diagnosis: std::sync::Arc::new(diagnosis::store::DiagnosisStore::new()),
                context_segments_enabled: app_state::context_segments_from_env(),
                wire_prefixes: Mutex::new(std::collections::VecDeque::new()),
            });
            // Startup diagnosis reads local state and the LARM catalog without
            // allocating a Connection or blocking the window setup.
            diagnosis::runner::spawn_startup(app.handle().clone());
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
            providers::reachability_watcher::spawn(&app.state::<AppState>());
            adaptive_improvement::start_worker(
                app.state::<AppState>().sqlite_writer.clone(),
                app.state::<AppState>().data_directory.clone(),
            );
            runtime::image_input::sweep(&app.state::<AppState>().data_directory);
            schedule::hydrate(&app.state::<AppState>());
            schedule::start_loop(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            let tauri::WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };
            // The fetch plugin closes its short-lived worker WebViews after each request.
            // Only closing the application window may cancel conversation runs.
            if window.label() != "main" {
                return;
            }
            let state = window.state::<AppState>();
            window_size::save(window, &state.data_directory);
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
            }
        });
}
#[cfg(feature = "quality-eval-harness")]
pub use memory::personal_state::live_harness as personal_state_harness;
