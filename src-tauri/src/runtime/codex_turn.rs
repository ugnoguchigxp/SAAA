use rusqlite::{params, OptionalExtension};
use std::fs;
use std::sync::Arc;

use crate::ipc_contract::RuntimeEvent;
use crate::persistence::{load_codex_settings, load_routing_settings};
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{
    database_error, send_runtime_terminal_event, update_runtime_provider, AppState,
    RunCancellation, StartTurnInput, TurnCompletion, TurnExecutionFailure,
};

#[cfg(test)]
pub(crate) use super::codex_persist::persist_codex_thread;
pub(crate) use super::codex_persist::{persist_codex_failure, persist_codex_success};
#[cfg(test)]
pub(crate) use super::codex_process::receive_supervised_codex_result;
pub(crate) use super::codex_process::{run_codex_turn_process, run_codex_turn_process_with_policy};

pub(crate) async fn execute_codex_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    policy_override: Option<crate::runtime::contracts::RunSupervisionPolicy>,
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
    let (settings, timeout_ms, existing_thread_id) = state.sqlite_readers.read(|connection| {
        let settings = load_codex_settings(connection)?;
        let routing = load_routing_settings(connection)?;
        let thread_id = connection
            .query_row(
                "SELECT thread_id FROM codex_threads WHERE conversation_id = ?1 AND workspace_path = ?2",
                params![input.conversation_id, workspace.to_string_lossy()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(database_error)?;
        Ok((settings, routing.coding_assist.timeout_ms, thread_id))
    })?;
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
    let on_event_for_worker = on_event.clone_box();
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
                on_event_for_worker.as_ref(),
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
                on_event_for_worker.as_ref(),
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
                presentation: crate::ipc_contract::VoicePresentationDecision {
                    decision: "silent".to_string(),
                    reason_code: "route_blocked".to_string(),
                },
                voice_policy: None,
            });
            Ok(TurnCompletion)
        }
        Err(failure) => {
            let cancelled =
                failure.code == crate::runtime::contracts::RunFailureCode::UserCancelled;
            let mut error = TurnExecutionFailure {
                code: failure.code,
                message: failure.message,
                supervisor_version: Some(crate::runtime::contracts::SUPERVISOR_VERSION),
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
