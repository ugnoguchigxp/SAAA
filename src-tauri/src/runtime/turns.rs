use rusqlite::params;
use std::fs;
use std::sync::Arc;

#[path = "start_turn.rs"]
pub(crate) mod command;

use super::event_hub::RuntimeEventSender;
use crate::ipc_contract::{RuntimeEvent, RuntimeFailureCode};
use crate::persistence::conversations::validate_conversation_write_target;
use crate::redact::{bounded_text, redact_runtime_text};
use crate::{
    database_error, execute_codex_turn, memory, new_id, now_iso, situation, AppState,
    RunCancellation, StartTurnInput, TurnExecutionFailure,
};

pub(crate) async fn execute_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    codex_policy_override: Option<crate::runtime::contracts::RunSupervisionPolicy>,
) -> Result<(), TurnExecutionFailure> {
    let task_mode = match prepare_runtime_run(state, input) {
        Ok(task_mode) => task_mode,
        Err(message) => {
            let _ = on_event.send(RuntimeEvent::Failed {
                run_id: input.run_id.clone(),
                code: RuntimeFailureCode::RuntimeError,
                message: redact_runtime_text(&message),
                recovery: "Review the conversation and runtime state, then retry.".to_string(),
            });
            return Err(TurnExecutionFailure::unsupervised(
                crate::runtime::contracts::RunFailureCode::InternalError,
                message,
            ));
        }
    };
    crate::steward::on_user_message(state, input);
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
                    || error.code == crate::runtime::contracts::RunFailureCode::UserCancelled;
                finish_supervised_runtime_run(
                    state,
                    &input.run_id,
                    if cancelled { "cancelled" } else { "failed" },
                    Some(if cancelled {
                        crate::runtime::contracts::RunFailureCode::UserCancelled
                    } else {
                        error.code
                    }),
                    error.supervisor_version,
                    error.last_progress_at.as_deref(),
                    Some(&error.message),
                )
                .map_err(|message| {
                    TurnExecutionFailure::unsupervised(
                        crate::runtime::contracts::RunFailureCode::InternalError,
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

    if crate::runtime::capability_commands::handle_user_turn(
        state,
        input,
        on_event,
        cancellation.clone(),
    )
    .await?
    {
        state
            .situation
            .set_conversation_state(situation::contracts::ConversationState::Idle);
        return Ok(());
    }

    // Tool-selection extraction runs once the input message has a persistent ID and before the
    // first provider request. It only runs in discovery mode; the legacy path is unchanged.
    if state.tool_selection.discovery_configured() {
        let input_message_id = state
            .sqlite_readers
            .read(|connection| {
                connection
                    .query_row(
                        "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                        rusqlite::params![input.run_id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .map_err(|error| error.to_string())
            })
            .ok()
            .flatten();
        if let Ok(principal) =
            crate::tool_selection::service::ensure_principal(&state.sqlite_writer)
        {
            let context =
                crate::tool_selection::RequestContext::new(&principal, &input.conversation_id)
                    .with_run(Some(input.run_id.clone()))
                    .with_message(input_message_id);
            let _ = state
                .tool_selection
                .begin_turn(&context, &input.content)
                .await;
        }
    }

    let response_task =
        crate::runtime::voice_response::start(state, input, on_event, cancellation.clone());
    let result = super::conversation_turn::execute_conversation_turn(
        state,
        input,
        on_event,
        cancellation.clone(),
    )
    .await;
    if let Some(task) = response_task {
        task.abort();
        let _ = task.await;
    }
    let finalization = match &result {
        Ok(message) => {
            crate::runtime::voice_response::complete(
                state,
                input,
                on_event,
                cancellation.clone(),
                message,
            )
            .await
        }
        Err(error)
            if cancellation.is_cancelled()
                || error.code == crate::runtime::contracts::RunFailureCode::UserCancelled =>
        {
            let finalization = finish_supervised_runtime_run(
                state,
                &input.run_id,
                "cancelled",
                Some(crate::runtime::contracts::RunFailureCode::UserCancelled),
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
                Some(error.code),
                None,
                None,
                Some(&error.message),
            );
            if finalization.is_ok() {
                let _ = on_event.send(RuntimeEvent::Failed {
                    run_id: input.run_id.clone(),
                    code: public_failure_code(error.code),
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
            crate::runtime::contracts::RunFailureCode::InternalError,
            message,
        )
    })?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let (status, message_id) = match &result {
        Ok(message) => ("completed", Some(message.id.as_str())),
        Err(error)
            if cancellation.is_cancelled()
                || error.code == crate::runtime::contracts::RunFailureCode::UserCancelled =>
        {
            ("cancelled", None)
        }
        Err(_) => ("failed", None),
    };
    state.sqlite_writer.write(|connection| {
        crate::role_routing::repository::record_provider_turn_finish(
            connection,
            &input.run_id,
            status,
            message_id,
            now_ms,
        )
    })?;
    result.map(|_| ())
}

pub(crate) fn send_runtime_terminal_event(
    on_event: &dyn RuntimeEventSender,
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
            code: public_failure_code(error.code),
            message: redact_runtime_text(&error.message),
            recovery: error.code.recovery().to_string(),
        });
    }
}

fn public_failure_code(code: crate::runtime::contracts::RunFailureCode) -> RuntimeFailureCode {
    match code {
        crate::runtime::contracts::RunFailureCode::ConfigurationError => {
            RuntimeFailureCode::ConfigurationError
        }
        crate::runtime::contracts::RunFailureCode::ChildStartFailed => {
            RuntimeFailureCode::ChildStartFailed
        }
        crate::runtime::contracts::RunFailureCode::RequestTimeout => {
            RuntimeFailureCode::RequestTimeout
        }
        crate::runtime::contracts::RunFailureCode::ProgressTimeout => {
            RuntimeFailureCode::ProgressTimeout
        }
        crate::runtime::contracts::RunFailureCode::TerminalTimeout => {
            RuntimeFailureCode::TerminalTimeout
        }
        crate::runtime::contracts::RunFailureCode::HardTimeout => RuntimeFailureCode::HardTimeout,
        crate::runtime::contracts::RunFailureCode::ChildExited => RuntimeFailureCode::ChildExited,
        crate::runtime::contracts::RunFailureCode::ProtocolError => {
            RuntimeFailureCode::ProtocolError
        }
        crate::runtime::contracts::RunFailureCode::PolicyViolation => {
            RuntimeFailureCode::PolicyViolation
        }
        crate::runtime::contracts::RunFailureCode::ProviderError => {
            RuntimeFailureCode::ProviderError
        }
        crate::runtime::contracts::RunFailureCode::ResponseTooLarge => {
            RuntimeFailureCode::ResponseTooLarge
        }
        crate::runtime::contracts::RunFailureCode::InternalError
        | crate::runtime::contracts::RunFailureCode::AppRestarted => {
            RuntimeFailureCode::InternalError
        }
        crate::runtime::contracts::RunFailureCode::UserCancelled => {
            RuntimeFailureCode::RuntimeError
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_supervised_runtime_run(
    state: &AppState,
    run_id: &str,
    status: &str,
    failure_code: Option<crate::runtime::contracts::RunFailureCode>,
    supervisor_version: Option<&str>,
    last_progress_at: Option<&str>,
    error: Option<&str>,
) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
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
                    failure_code.map(crate::runtime::contracts::RunFailureCode::as_str),
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
    })
}

pub(crate) fn prepare_runtime_run(
    state: &AppState,
    input: &StartTurnInput,
) -> Result<String, String> {
    memory::personal_state::worker::interrupt();
    let _policy = state
        .interaction_policy
        .lock()
        .map_err(|_| "Interaction policy lock unavailable".to_string())?;
    let task_mode: String = state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT task_mode FROM conversations WHERE id = ?1",
                params![input.conversation_id],
                |row| row.get(0),
            )
            .map_err(|_| "Conversation does not exist".to_string())
    })?;
    validate_conversation_write_target(&input.conversation_id, &task_mode)?;
    if task_mode == "coding" {
        let workspace = input
            .workspace_path
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "Select a workspace before starting a Codex turn".to_string())?;
        let workspace = fs::canonicalize(workspace)
            .map_err(|_| "The selected Codex workspace does not exist".to_string())?;
        if !workspace.is_dir() {
            return Err("The selected Codex workspace is not a directory".to_string());
        }
    } else if input.workspace_path.is_some() {
        return Err("Normal conversation turns cannot include a workspace".to_string());
    }
    if task_mode == "conversation" {
        memory::context_window::validate_current_instruction(input.content.trim())?;
    }
    let route_kind = if task_mode == "coding" {
        "coding.assist"
    } else {
        "conversation.respond"
    };
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        if task_mode == "conversation"
            && crate::persistence::load_role_routing_settings(&transaction)?.enabled
        {
            let active_root: bool = transaction
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM rr_roots WHERE conversation_id=?1 AND phase IN ('queued','responding','draining'))",
                    params![input.conversation_id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if active_root {
                return Err("Role-routing conversation is busy; wait for the current response or cancel it".to_string());
            }
        }
        let now = now_iso();
        let new_message = input.retry_input_message_id.is_none();
        let input_message_id = if let Some(message_id) = input.retry_input_message_id.as_deref() {
            crate::validate_identifier(message_id, "retry input message id")?;
            let retryable: bool = transaction
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM conversation_messages m
                       WHERE m.id = ?1 AND m.conversation_id = ?2 AND m.role = 'user'
                         AND m.content = ?3
                         AND EXISTS(
                           SELECT 1 FROM runtime_runs r
                           WHERE r.input_message_id = m.id AND r.status = 'failed'
                         )
                     )",
                    params![message_id, input.conversation_id, input.content.trim()],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if !retryable {
                return Err("Only a failed conversation response can be retried".to_string());
            }
            message_id.to_string()
        } else {
            let message_id = new_id("message");
            transaction
                .execute(
                    "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                     VALUES (?1, ?2, 'user', ?3, ?4)",
                    params![message_id, input.conversation_id, input.content.trim(), now],
                )
                .map_err(database_error)?;
            message_id
        };
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
                        Some(crate::runtime::contracts::SUPERVISOR_VERSION)
                    } else {
                        None
                    },
                    input_message_id,
                ],
            )
            .map_err(database_error)?;
        crate::runtime::context::scope::resolve(
            &transaction,
            input,
            &input_message_id,
            new_message,
        )?;
        if task_mode == "conversation" {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            let routing_started = crate::role_routing::repository::record_provider_turn_start_in_transaction(
                &transaction,
                &input.run_id,
                &input.conversation_id,
                now_ms,
            )?;
            if routing_started {
                crate::role_routing::coordinator::apply_in_transaction(
                    &transaction,
                    &input.run_id,
                    crate::role_routing::reducer::Event::Start,
                    now_ms,
                )?;
            }
        }
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
        Ok(())
    })?;
    Ok(task_mode)
}

#[cfg(test)]
pub(crate) fn finish_runtime_run(
    state: &AppState,
    run_id: &str,
    status: &str,
    error: Option<&str>,
) -> Result<(), String> {
    state.sqlite_writer.write(|connection| {
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
    })
}
