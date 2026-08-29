use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crate::ipc_contract::{ConversationMessage, RuntimeEvent};
use crate::persistence::{load_codex_settings, load_routing_settings};
use crate::process_guard::ProcessGuard;
use crate::redact::{bounded_text, redact_runtime_text};
use crate::{
    database_error, memory, new_id, now_iso, send_runtime_terminal_event, spawn_codex_app_server,
    update_runtime_provider, validate_identifier, write_codex_handshake, write_codex_message,
    AppState, CodexReaderMessage, CodexTurnFailure, CodexTurnOutcome, RunCancellation,
    StartTurnInput, TurnCompletion, TurnExecutionFailure, CODEX_READ_ONLY_SYSTEM_CONTEXT,
    MAX_CODEX_STDOUT_BYTES,
};

pub(crate) async fn execute_codex_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
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

pub(crate) fn upsert_codex_thread(
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
pub(crate) fn persist_codex_thread(
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

pub(crate) fn persist_codex_success(
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
                crate::runtime::contracts::SUPERVISOR_VERSION,
                outcome.last_progress_at,
                input.run_id
            ],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Runtime run was already finalized".to_string());
    }
    if memory::control_plane::memory_enabled()
        && transaction
            .execute_batch("SAVEPOINT memory_turn_enqueue")
            .is_ok()
    {
        let memory_now = now_iso();
        let memory_result = transaction
            .query_row(
                "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                params![input.run_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(database_error)
            .and_then(|input_message_id| {
                memory::control_plane::record_completed_turn(
                    &transaction,
                    &input_message_id,
                    &message.id,
                    &memory_now,
                )
            });
        match memory_result {
            Ok(_) => {
                if transaction
                    .execute_batch("RELEASE memory_turn_enqueue")
                    .is_ok()
                {
                    let _ = memory::control_plane::record_decision_event(
                        &transaction,
                        "turn-enqueue",
                        "queued",
                        4,
                        &memory_now,
                    );
                }
            }
            Err(_) => {
                let _ = transaction
                    .execute_batch("ROLLBACK TO memory_turn_enqueue; RELEASE memory_turn_enqueue");
                let _ = memory::control_plane::record_decision_event(
                    &transaction,
                    "turn-enqueue",
                    "failed",
                    0,
                    &memory_now,
                );
            }
        }
    }
    transaction.commit().map_err(database_error)?;
    Ok(message)
}

pub(crate) fn persist_codex_failure(
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
                crate::runtime::contracts::SUPERVISOR_VERSION,
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
pub(crate) fn run_codex_turn_process(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    timeout_ms: u64,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: &RunCancellation,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    let policy = crate::runtime::contracts::RunSupervisionPolicy::for_route(timeout_ms).map_err(
        |message| CodexTurnFailure {
            thread_id: existing_thread_id.map(str::to_string),
            message,
            code: crate::runtime::contracts::RunFailureCode::ConfigurationError,
            last_progress_at: None,
        },
    )?;
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
pub(crate) fn run_codex_turn_process_with_policy(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    policy: crate::runtime::contracts::RunSupervisionPolicy,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: &RunCancellation,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    use crate::runtime::codex_app_server::{CodexEventProjector, ProjectedCodexEvent};
    use crate::runtime::contracts::{RunFailureCode, RunOutcome, RunSignal, TerminalStatus};
    use crate::runtime::supervisor::RunSupervisor;

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

pub(crate) fn elapsed_millis(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn supervisor_wait_duration(
    supervisor: &crate::runtime::supervisor::RunSupervisor,
    now_ms: u64,
) -> Duration {
    let remaining_ms = supervisor
        .next_deadline_ms()
        .map(|deadline| deadline.saturating_sub(now_ms))
        .unwrap_or(100);
    Duration::from_millis(remaining_ms.clamp(1, 100))
}

pub(crate) fn apply_supervisor_actions(
    actions: &[crate::runtime::contracts::SupervisorAction],
    child: &mut ProcessGuard,
    stdin: &mut impl Write,
    thread_id: &str,
    turn_id: &str,
    pending_outcome: Option<crate::runtime::contracts::RunOutcome>,
) -> Option<crate::runtime::contracts::RunOutcome> {
    use crate::runtime::contracts::SupervisorAction;
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
                    outcome =
                        pending_outcome.or(Some(crate::runtime::contracts::RunOutcome::Failed(
                            crate::runtime::contracts::RunFailureCode::InternalError,
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

pub(crate) fn supervisor_outcome(
    outcome: crate::runtime::contracts::RunOutcome,
    thread_id: &str,
    content: &str,
    failure_detail: Option<String>,
    last_progress_at: Option<String>,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    use crate::runtime::contracts::{RunFailureCode, RunOutcome};
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

pub(crate) fn receive_supervised_codex_result(
    receiver: &mpsc::Receiver<CodexReaderMessage>,
    request_id: u64,
    supervisor: &mut crate::runtime::supervisor::RunSupervisor,
    origin: std::time::Instant,
    cancellation: &RunCancellation,
) -> Result<Value, crate::runtime::contracts::RunFailureCode> {
    use crate::runtime::contracts::{RunFailureCode, RunOutcome, RunSignal, SupervisorAction};

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

pub(crate) fn request_failure_message(
    code: crate::runtime::contracts::RunFailureCode,
) -> &'static str {
    use crate::runtime::contracts::RunFailureCode;
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
