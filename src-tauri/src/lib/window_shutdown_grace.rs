use super::{
    AppSnapshot, ListMessagesInput, LocalArtifactResult, ProviderTestResult,
    SaveSettingsDocumentsInput, SetVoiceListeningEnabledInput, SettingsDocument,
    TestProviderInput, database_error, now_iso, validate_identifier, spawn_situation_monitor,
};
use crate::app_state::{AppState, ProviderProbeStatus, RunCancellation};
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
use crate::ipc_contract::ConversationMessagePage;
use crate::persistence::{list_message_page_from_connection, SqliteReaders, SqliteWriter};
use crate::voice::streaming_asr::{
    append_voice_asr_audio, commit_voice_asr_utterance, start_voice_asr_session,
    stop_voice_asr_session, AsrSessionManager,
};
use crate::voice_behavior::{
    get_conversation_voice_policy, reset_conversation_voice_policy,
    update_conversation_voice_policy,
};
use crate::voice_commands::{
    delete_voice_enrollment_sample, delete_voice_profile, get_voice_profile_snapshot,
    read_voice_enrollment_sample, save_voice_enrollment_sample, set_target_speaker_filter_enabled,
    stage_audio_upload, stop_tts,
};
#[cfg(test)]
use rusqlite::Connection;
#[cfg(test)]
use std::fs;
#[cfg(test)]
use crate::persistence::conversations::list_messages_from_connection;
#[cfg(test)]
use crate::persistence::schema::initialize_database;
#[cfg(test)]
pub(crate) use crate::runtime::codex_turn::{
    persist_codex_thread, receive_supervised_codex_result, run_codex_turn_process,
    run_codex_turn_process_with_policy,
};
#[cfg(test)]
pub(crate) use crate::runtime::turns::finish_runtime_run;
#[cfg(test)]
pub(crate) use crate::runtime::turns::prepare_runtime_run;
#[cfg(test)]
use crate::ipc_contract::ConversationMessage;
#[cfg(test)]
use crate::ipc_contract::RuntimeEvent;
pub(crate) const WINDOW_SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
pub(crate) const DYNAMIC_LAN_PROVIDER_ID: &str = "lan-llm-dynamic";
pub(crate) const QWEN_DIRECT_PROVIDER_ID: &str = "lan-qwen-direct";
pub(crate) const DEFAULT_DYNAMIC_LAN_HOST: &str = "gnosis.local";
pub(crate) const DEFAULT_AGENT_NAME: &str = "SAAA";
pub(crate) const DEFAULT_USER_NAME: &str = "";
pub(crate) const PRIMARY_CONVERSATION_ID: &str = "conversation_primary";
pub(crate) const PRIMARY_CONVERSATION_TITLE: &str = "SAAAとの会話";
pub(crate) const CODEX_READ_ONLY_SYSTEM_CONTEXT: &str = include_str!("../../../.s11tnext/codex-read-only.txt");
#[tauri::command]
pub(super) fn frontend_ready(state: tauri::State<'_, AppState>) -> Result<(), String> {
    crate::app_paths::frontend_ready(&state)
}
#[tauri::command]
pub(super) fn get_app_snapshot(state: tauri::State<'_, AppState>) -> Result<AppSnapshot, String> {
    crate::persistence::app_commands::get_app_snapshot(&state)
}
#[tauri::command]
pub(super) fn get_situation_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<crate::situation::contracts::SituationSnapshot, String> {
    state.situation.snapshot_locked(&state.sqlite_readers)
}
#[tauri::command]
pub(super) fn set_situation_monitoring(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<crate::situation::contracts::SituationSnapshot, String> {
    state
        .situation
        .set_monitoring(&state.sqlite_writer, enabled)?;
    if enabled {
        spawn_situation_monitor(state.sqlite_writer.clone(), state.situation.clone());
    }
    get_situation_snapshot(state)
}
#[tauri::command]
pub(super) fn report_owned_signal(
    state: tauri::State<'_, AppState>,
    input: crate::situation::contracts::OwnedSignalInput,
) -> Result<(), String> {
    state.situation.report_owned(input)
}
#[tauri::command]
pub(super) fn submit_situation_feedback(
    state: tauri::State<'_, AppState>,
    input: crate::situation::contracts::SituationFeedbackInput,
) -> Result<crate::situation::contracts::SituationSnapshot, String> {
    state
        .sqlite_writer
        .write(|connection| crate::situation::repository::submit_feedback(connection, &input))?;
    state.situation.snapshot_locked(&state.sqlite_readers)
}
#[tauri::command]
pub(super) fn clear_situation_history(
    state: tauri::State<'_, AppState>,
) -> Result<crate::situation::contracts::SituationSnapshot, String> {
    state.situation.clear_history(&state.sqlite_writer)?;
    state.situation.snapshot_locked(&state.sqlite_readers)
}
#[tauri::command]
pub(super) fn get_situation_review_snapshot(
    state: tauri::State<'_, AppState>,
) -> Result<crate::situation::contracts::SituationReviewSnapshot, String> {
    state.sqlite_readers.read(|connection| {
        Ok(crate::situation::contracts::SituationReviewSnapshot {
            active_profile: crate::situation::calibration::active_profile(connection)?,
            quality: crate::situation::repository::quality_metrics(connection)?,
            feedback_queue: crate::situation::repository::feedback_queue(connection)?,
            latest_run: crate::situation::calibration::latest_run(connection)?,
            candidates: crate::situation::calibration::candidates(connection)?,
        })
    })
}
#[tauri::command]
pub(super) fn create_situation_calibration_candidate(
    state: tauri::State<'_, AppState>,
    parameters: crate::situation::contracts::CalibrationParameters,
) -> Result<crate::situation::calibration::CalibrationProfile, String> {
    state
        .sqlite_writer
        .write(|connection| crate::situation::calibration::create_candidate(connection, parameters))
}
#[tauri::command]
pub(super) async fn run_situation_calibration(
    state: tauri::State<'_, AppState>,
    profile_id: String,
) -> Result<crate::situation::calibration::CalibrationRun, String> {
    validate_identifier(&profile_id, "calibration profile id")?;
    let readers = state.sqlite_readers.clone();
    let writer = state.sqlite_writer.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let profile = readers
            .read(|connection| crate::situation::calibration::profile_by_id(connection, &profile_id))?;
        if profile.status != "candidate" {
            return Err("Only candidate profiles can be replayed".to_string());
        }
        let metrics = crate::situation::calibration::replay_metrics(&profile)?;
        writer.write(|connection| {
            crate::situation::calibration::save_run(
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
pub(super) fn decide_situation_calibration(
    state: tauri::State<'_, AppState>,
    profile_id: String,
    decision: String,
    reason_code: String,
) -> Result<crate::situation::contracts::SituationReviewSnapshot, String> {
    state.situation.decide_calibration(
        &state.sqlite_writer,
        &profile_id,
        &decision,
        &reason_code,
    )?;
    get_situation_review_snapshot(state)
}
#[tauri::command]
pub(super) fn export_diagnostics(state: tauri::State<'_, AppState>) -> Result<LocalArtifactResult, String> {
    crate::diagnostics::export_diagnostics(&state)
}
#[tauri::command]
pub(super) async fn load_tts_voice_catalog(
    state: tauri::State<'_, AppState>,
    input: crate::voice::cloud_tts::tts_catalog::LoadTtsVoiceCatalogInput,
) -> Result<crate::voice::cloud_tts::tts_catalog::TtsVoiceCatalog, String> {
    crate::voice::cloud_tts::tts_catalog::load_tts_voice_catalog(&state, input).await
}
#[tauri::command]
pub(super) async fn test_model_provider(
    state: tauri::State<'_, AppState>,
    input: TestProviderInput,
) -> Result<ProviderTestResult, String> {
    crate::providers::probe::test_model_provider(&state, input).await
}
#[tauri::command]
pub(super) async fn resolve_service_harness(
    address: String,
) -> Result<crate::providers::service_harness::HarnessResolution, String> {
    crate::providers::service_harness::resolve_with_legacy_llm(&address).await
}
#[tauri::command]
pub(super) fn set_provider_api_key(
    state: tauri::State<'_, AppState>,
    input: crate::credentials::SetProviderApiKeyInput,
) -> Result<crate::credentials::ProviderCredentialState, String> {
    crate::credentials::set_api_key(&state.sqlite_readers, input)
}
#[tauri::command]
pub(super) fn delete_provider_api_key(
    provider_id: String,
) -> Result<crate::credentials::ProviderCredentialState, String> {
    crate::credentials::delete_api_key(provider_id)
}
#[tauri::command]
pub(super) fn get_provider_credential_state(
    provider_id: String,
) -> Result<crate::credentials::ProviderCredentialState, String> {
    crate::credentials::credential_state(provider_id)
}
#[tauri::command]
pub(super) fn record_frontend_audit_event(
    state: tauri::State<'_, AppState>,
    input: crate::persistence::audit::FrontendAuditEventInput,
) -> Result<(), String> {
    crate::persistence::audit::record_frontend_event(&state, &input)
}
#[tauri::command]
pub(super) fn cancel_run(
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
    let event = crate::persistence::audit::FrontendAuditEventInput {
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
            crate::persistence::audit::AuditAttributeValue::Tag(reason.into()),
        )]),
    };
    let _ = crate::persistence::audit::record_frontend_event(&state, &event);
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
pub(super) fn save_settings_documents(
    state: tauri::State<'_, AppState>,
    input: SaveSettingsDocumentsInput,
) -> Result<Vec<SettingsDocument>, String> {
    crate::persistence::app_commands::save_settings_documents(&state, input)
}
#[tauri::command]
pub(super) fn set_voice_listening_enabled(
    state: tauri::State<'_, AppState>,
    input: SetVoiceListeningEnabledInput,
) -> Result<SettingsDocument, String> {
    crate::persistence::settings::set_voice_listening_enabled(&state, input.enabled)
}
#[tauri::command]
pub(super) async fn list_messages(
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
pub(super) fn build_capability_service(
    sqlite_writer: Arc<SqliteWriter>,
    data_directory: &std::path::Path,
) -> crate::generated_capabilities::service::CapabilityService {
    let config = std::env::var_os("SAAA_LLANG_RUNTIME_CONFIG")
        .map(PathBuf::from)
        .and_then(|path| {
            crate::generated_capabilities::host::runtime_bundle::load_config(&path)
                .map_err(|error| {
                    eprintln!("generated capability runtime config rejected: {error}");
                })
                .ok()
        });
    let ledger_directory = data_directory
        .join("generated-capabilities")
        .join("acceptance");
    crate::generated_capabilities::service::CapabilityService::build(
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

pub(super) fn shutdown_app_state(state: &AppState) {
    crate::memory::personal_state::worker::interrupt();
    crate::coding::commands::shutdown(state);
    state.generated_capabilities.shutdown();
    // Stop admitting new external MCP calls and close sessions. The task is spawned because the
    // close path is synchronous; in-flight calls keep their indeterminate outcome.
    if let Some(manager) = state.tool_selection.mcp_manager() {
        tauri::async_runtime::spawn(async move { manager.shutdown().await });
    }
    // Stop the published MCP listener; the D4 manager and result writer stay alive for their tasks.
    crate::tool_selection::mcp_server::shutdown_slot(&state.mcp_server);
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
        .set_microphone_state(crate::situation::contracts::MicrophoneState::Inactive);
    state
        .situation
        .set_audio_state(crate::situation::contracts::AudioState::Silent);
}
