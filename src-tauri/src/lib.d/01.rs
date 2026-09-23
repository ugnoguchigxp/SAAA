#[cfg(test)]
use rusqlite::Connection;
#[cfg(test)]
use std::fs;
#[cfg(test)]
use persistence::conversations::list_messages_from_connection;
#[cfg(test)]
use persistence::schema::initialize_database;
#[cfg(test)]
pub(crate) use runtime::codex_turn::{
    persist_codex_thread, receive_supervised_codex_result, run_codex_turn_process,
    run_codex_turn_process_with_policy,
};
#[cfg(test)]
pub(crate) use runtime::turns::finish_runtime_run;
#[cfg(test)]
pub(crate) use runtime::turns::prepare_runtime_run;
#[cfg(test)]
use ipc_contract::ConversationMessage;
#[cfg(test)]
use ipc_contract::RuntimeEvent;
const WINDOW_SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
const DYNAMIC_LAN_PROVIDER_ID: &str = "lan-llm-dynamic";
const QWEN_DIRECT_PROVIDER_ID: &str = "lan-qwen-direct";
const DEFAULT_DYNAMIC_LAN_HOST: &str = "gnosis.local";
const DEFAULT_AGENT_NAME: &str = "SAAA";
const DEFAULT_USER_NAME: &str = "";
const PRIMARY_CONVERSATION_ID: &str = "conversation_primary";
const PRIMARY_CONVERSATION_TITLE: &str = "SAAAとの会話";
const CODEX_READ_ONLY_SYSTEM_CONTEXT: &str = include_str!("../../../.s11tnext/codex-read-only.txt");
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
async fn load_tts_voice_catalog(
    state: tauri::State<'_, AppState>,
    input: crate::voice::cloud_tts::tts_catalog::LoadTtsVoiceCatalogInput,
) -> Result<crate::voice::cloud_tts::tts_catalog::TtsVoiceCatalog, String> {
    crate::voice::cloud_tts::tts_catalog::load_tts_voice_catalog(&state, input).await
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
fn cancel_run(
    state: tauri::State<'_, AppState>,
    run_id: String,
    reason: Option<String>,
) -> Result<(), String> {
    validate_identifier(&run_id, "run id")?;
    let reason = match reason.as_deref() {
        Some("invalid-ipc-event") => "invalid-ipc-event",
        Some("conversation-unmounted") => "conversation-unmounted",
        Some("replaced-by-new-prompt") => "replaced-by-new-prompt",
        Some("user-stop") => "user-stop",
        _ => "unknown",
    };
    let event = persistence::audit::FrontendAuditEventInput {
        component: "conversation".into(),
        event_name: "run-cancel-requested".into(),
        phase: "request".into(),
        outcome: None,
        correlation_id: Some(run_id.clone()),
        causation_id: None,
        conversation_id: None,
        runtime_run_id: Some(run_id.clone()),
        session_id: None,
        subject_id: Some(run_id.clone()),
        failure_code: None,
        attributes: std::collections::BTreeMap::from([(
            "reason".into(),
            persistence::audit::AuditAttributeValue::Tag(reason.into()),
        )]),
    };
    let _ = persistence::audit::record_frontend_event(&state, &event);
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
