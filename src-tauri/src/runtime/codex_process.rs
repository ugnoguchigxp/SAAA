#[path = "codex_output_reader.rs"]
mod output_reader;
use serde_json::{json, Value};
use std::sync::mpsc;

use crate::ipc_contract::RuntimeEvent;
use crate::process_guard::ProcessGuard;
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{
    now_iso, spawn_codex_app_server, validate_identifier, write_codex_handshake,
    write_codex_message, CodexReaderMessage, CodexTurnFailure, CodexTurnOutcome, RunCancellation,
    CODEX_READ_ONLY_SYSTEM_CONTEXT,
};

pub(crate) use super::codex_supervise::{
    apply_supervisor_actions, elapsed_millis, receive_supervised_codex_result,
    request_failure_message, supervisor_outcome, supervisor_wait_duration,
};

fn developer_instructions(host_context: &str) -> String {
    if host_context.trim().is_empty() {
        return CODEX_READ_ONLY_SYSTEM_CONTEXT.to_string();
    }
    format!("{CODEX_READ_ONLY_SYSTEM_CONTEXT}\n\n{host_context}")
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_codex_turn_process(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    timeout_ms: u64,
    on_event: &dyn RuntimeEventSender,
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
    run_codex_turn_process_with_policy_and_context(
        run_id,
        prompt,
        workspace,
        model,
        existing_thread_id,
        "",
        policy,
        on_event,
        cancellation,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_codex_turn_process_with_policy(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    policy: crate::runtime::contracts::RunSupervisionPolicy,
    on_event: &dyn RuntimeEventSender,
    cancellation: &RunCancellation,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    run_codex_turn_process_with_policy_and_context(
        run_id,
        prompt,
        workspace,
        model,
        existing_thread_id,
        "",
        policy,
        on_event,
        cancellation,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_codex_turn_process_with_policy_and_context(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    host_context: &str,
    policy: crate::runtime::contracts::RunSupervisionPolicy,
    on_event: &dyn RuntimeEventSender,
    cancellation: &RunCancellation,
) -> Result<CodexTurnOutcome, CodexTurnFailure> {
    run_codex_turn_process_with_dispatch(
        run_id,
        prompt,
        workspace,
        model,
        existing_thread_id,
        host_context,
        policy,
        on_event,
        cancellation,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_codex_turn_process_with_dispatch(
    run_id: &str,
    prompt: &str,
    workspace: &std::path::Path,
    model: &str,
    existing_thread_id: Option<&str>,
    host_context: &str,
    policy: crate::runtime::contracts::RunSupervisionPolicy,
    on_event: &dyn RuntimeEventSender,
    cancellation: &RunCancellation,
    mut dispatch: Option<&mut super::codex_context::Dispatch>,
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
    let (receiver, stdout_reader) = output_reader::spawn(stdout);
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
        let fresh_context = dispatch
            .as_deref_mut()
            .map(|d| d.prepare())
            .transpose()
            .map_err(|message| CodexTurnFailure {
                thread_id: thread_id.clone(),
                message,
                code: RunFailureCode::ConfigurationError,
                last_progress_at: None,
            })?;
        let host_context = fresh_context.as_deref().unwrap_or(host_context);
        let mut params = json!({
            "cwd": workspace_text,
            "approvalPolicy": "never",
            "sandbox": "read-only",
            "config": {
                "web_search": "disabled",
                "mcp_servers": {},
                "sandbox_workspace_write": { "network_access": false }
            },
            "developerInstructions": developer_instructions(host_context)
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
        let thread_body = json!({ "method": method, "id": 2, "params": params });
        write_codex_message(&mut stdin, thread_body.clone()).map_err(|message| {
            CodexTurnFailure {
                thread_id: thread_id.clone(),
                code: RunFailureCode::ChildExited,
                message,
                last_progress_at: None,
            }
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
        let turn_body = json!({
            "method": "turn/start",
            "id": 3,
            "params": {
                "threadId": thread_id,
                "input": [{ "type": "text", "text": prompt, "text_elements": [] }],
                "cwd": workspace_text,
                "approvalPolicy": "never"
            }
        });
        if let Some(dispatch) = dispatch.as_deref_mut() {
            dispatch
                .dispatch(&thread_body, &turn_body)
                .map_err(|message| CodexTurnFailure {
                    thread_id: Some(thread_id.clone()),
                    message,
                    code: RunFailureCode::ConfigurationError,
                    last_progress_at: None,
                })?;
        }
        write_codex_message(&mut stdin, turn_body).map_err(|message| CodexTurnFailure {
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
        if let Some(dispatch) = dispatch.as_deref() {
            let _ = dispatch.finish(false);
        }
        return Err(CodexTurnFailure {
            thread_id,
            message: "Codex output reader stopped unexpectedly".to_string(),
            code: RunFailureCode::InternalError,
            last_progress_at,
        });
    }
    if let Some(dispatch) = dispatch {
        dispatch
            .finish(result.is_ok())
            .map_err(|message| CodexTurnFailure {
                thread_id,
                message,
                code: RunFailureCode::ConfigurationError,
                last_progress_at,
            })?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::developer_instructions;

    #[test]
    fn wd_09_host_snapshot_is_kept_at_the_developer_boundary() {
        let instructions = developer_instructions(
            "HOST_STATE_SNAPSHOT <host-state-snapshot>{}</host-state-snapshot>",
        );
        assert!(instructions.contains("Operate read-only"));
        assert!(instructions.contains("HOST_STATE_SNAPSHOT"));
    }
}
