use rusqlite::params;
use std::fs;
use std::sync::Arc;

#[path = "start_turn.rs"]
pub(crate) mod command;
#[path = "conversation_context.rs"]
mod conversation_context;

use super::event_hub::RuntimeEventSender;
use crate::ipc_contract::{ConversationMessage, RuntimeEvent, RuntimeFailureCode};
use crate::persistence::conversations::validate_conversation_write_target;
use crate::persistence::{
    load_codex_settings, load_model_providers, load_routing_settings, load_security_settings,
};
use crate::providers::routing::{
    apply_runtime_provider_gates, effective_conversation_route_ids, resolve_harness_llm_provider,
};
use crate::redact::{bounded_text, redact_runtime_text};
use crate::{
    begin_provider_session, database_error, execute_codex_turn,
    finish_dynamic_lan_provider_session, finish_larm_provider_session, finish_provider_session,
    memory, new_id, now_iso, persist_conversation_success, situation, stream_dynamic_lan_provider,
    stream_larm_provider, stream_model_provider, update_runtime_provider, AppState, CleanupOutcome,
    LarmStreamContext, ModelProviderSettings, ModelStreamContext, ProviderAttemptOutcome,
    ProviderFailureKind, ProviderOutputPersistence, RunCancellation, StartTurnInput,
    TurnExecutionFailure,
};
use conversation_context::compose_provider_history;

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

    let result = execute_conversation_turn(state, input, on_event, cancellation.clone())
        .await
        .map_err(|message| {
            TurnExecutionFailure::unsupervised(
                crate::runtime::contracts::RunFailureCode::ProviderError,
                message,
            )
        });
    let finalization = match &result {
        Ok(message) => {
            let (presentation, voice_policy) = crate::voice_behavior::completion_state(
                state,
                &input.run_id,
                &input.conversation_id,
            );
            let _ = on_event.send(RuntimeEvent::MessageCompleted {
                run_id: input.run_id.clone(),
                message: message.clone(),
                presentation,
                voice_policy,
            });
            Ok(())
        }
        Err(error) if cancellation.is_cancelled() => {
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
                None,
                None,
                None,
                Some(&error.message),
            );
            if finalization.is_ok() {
                let _ = on_event.send(RuntimeEvent::Failed {
                    run_id: input.run_id.clone(),
                    code: RuntimeFailureCode::RuntimeError,
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
            code: match error.code {
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
                crate::runtime::contracts::RunFailureCode::HardTimeout => {
                    RuntimeFailureCode::HardTimeout
                }
                crate::runtime::contracts::RunFailureCode::ChildExited => {
                    RuntimeFailureCode::ChildExited
                }
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
            },
            message: redact_runtime_text(&error.message),
            recovery: error.code.recovery().to_string(),
        });
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
    if task_mode == "coding" && state.meeting.blocks_tts() {
        return Err(
            "MEETING_POLICY_AGENT_BLOCKED: Coding Agent is disabled during a meeting.".to_string(),
        );
    }
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
        let now = now_iso();
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

pub(crate) async fn execute_conversation_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
) -> Result<ConversationMessage, String> {
    let (
        mut providers,
        route,
        security,
        identity,
        regional,
        loaded_context,
        configuration_fingerprint,
    ) = state.sqlite_readers.read(|connection| {
        let input_message_id: String = connection
            .query_row(
                "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                params![input.run_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let loaded_context =
            memory::context_window::load(connection, &input.conversation_id, &input_message_id)?;
        let identity = load_codex_settings(connection)?;
        let regional = crate::persistence::settings::regional_preferences::load(connection)?;
        let providers = load_model_providers(connection)?;
        let route = load_routing_settings(connection)?.conversation_respond;
        let configuration_fingerprint =
            crate::persistence::effective_route::conversation_configuration_fingerprint(
                &providers, &route,
            )?;
        Ok((
            providers,
            route,
            load_security_settings(connection)?,
            identity,
            regional,
            loaded_context,
            configuration_fingerprint,
        ))
    })?;
    let context_window = memory::context_window::compose(loaded_context)?;
    let context_health = context_window.health.clone();
    let _ = state.sqlite_writer.write(|connection| {
        memory::control_plane::record_projection_event(
            connection,
            context_health.status,
            context_health.projected_bytes,
            context_health.hard_limit_bytes,
            context_health.output_reserve_bytes,
            context_health.repair_count,
            &now_iso(),
        )
    });
    let history = compose_provider_history(
        &input.conversation_id,
        &identity.agent_name,
        &identity.user_name,
        &regional,
        &input.input_origin,
        &input.presentation_mode,
        context_window.messages,
    )?;
    let mut route = route;
    if route.source == "harness" {
        route.timeout_ms =
            resolve_harness_llm_provider(&mut providers, route.timeout_ms, cancellation.clone())
                .await?;
    }
    let reasoning_effort = providers.reasoning_effort.clone();
    let max_output_tokens = crate::providers::completion::DEFAULT_MAX_OUTPUT_TOKENS;
    let route_ids = apply_dynamic_lan_credential_gate(
        &providers,
        apply_runtime_provider_gates(
            &providers,
            effective_conversation_route_ids(&providers, &route, &security),
            &state.larm_gate,
        ),
        crate::providers::dynamic_lan::control_credential_available(),
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
        let session_id = begin_provider_session(
            state,
            &input.run_id,
            provider.id(),
            provider.kind(),
            &configuration_fingerprint,
        )?;
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
                    "Context {}: {}/{} input bytes, {} bytes output reserved, {} memory items, {} recent messages, {} continuity groups, {} loaded source messages omitted{}{}",
                    context_health.status,
                    context_health.projected_bytes,
                    context_health.hard_limit_bytes,
                    context_health.output_reserve_bytes,
                    context_health.memory_item_count,
                    context_health.recent_source_messages,
                    context_health.continuity_group_count,
                    context_health.omitted_loaded_source_messages,
                    if context_health.source_history_truncated {
                        ", older source history truncated"
                    } else {
                        ""
                    },
                    if context_health.repair_count > 0 {
                        ", minimal reconstruction applied"
                    } else {
                        ""
                    },
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
                    ModelStreamContext {
                        reasoning_effort: &reasoning_effort,
                        max_output_tokens,
                        input,
                        on_event,
                        cancellation: cancellation.clone(),
                        output_persistence: Some(ProviderOutputPersistence {
                            state,
                            session_id: &session_id,
                        }),
                    },
                )
                .await
            }
            ModelProviderSettings::Larm(provider) => {
                stream_larm_provider(
                    provider,
                    &history,
                    &reasoning_effort,
                    max_output_tokens,
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
            ModelProviderSettings::DynamicLan(provider) => {
                stream_dynamic_lan_provider(
                    provider,
                    &history,
                    route.timeout_ms,
                    cancellation.clone(),
                    ModelStreamContext {
                        reasoning_effort: &reasoning_effort,
                        max_output_tokens,
                        input,
                        on_event,
                        cancellation: cancellation.clone(),
                        output_persistence: Some(ProviderOutputPersistence {
                            state,
                            session_id: &session_id,
                        }),
                    },
                )
                .await
            }
            ModelProviderSettings::CloudAsr(_)
            | ModelProviderSettings::CloudTts(_)
            | ModelProviderSettings::SystemTts(_) => ProviderAttemptOutcome::Failed {
                kind: ProviderFailureKind::Contract,
                public_message: ProviderFailureKind::Contract.public_message(),
                output_started: false,
                cleanup: CleanupOutcome::NotApplicable,
            },
        };
        match outcome {
            ProviderAttemptOutcome::Completed { content, cleanup } => {
                if provider.kind() == "larm" {
                    finish_larm_provider_session(state, &session_id, "completed", None, cleanup)?;
                } else if matches!(&provider, ModelProviderSettings::DynamicLan(_)) {
                    finish_dynamic_lan_provider_session(
                        state,
                        &session_id,
                        "completed",
                        None,
                        cleanup,
                    )?;
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
                } else if matches!(&provider, ModelProviderSettings::DynamicLan(_)) {
                    finish_dynamic_lan_provider_session(
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
                } else if matches!(&provider, ModelProviderSettings::DynamicLan(_)) {
                    finish_dynamic_lan_provider_session(
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
                if !provider_route_fallback_allowed(&provider, kind, output_started) {
                    return Err(failure);
                }
                failures.push(failure);
            }
        }
    }
    if failures.len() == 1 {
        Err(failures.remove(0))
    } else {
        Err(format!(
            "Configured provider attempts failed. {}",
            failures.join("; ")
        ))
    }
}

fn apply_dynamic_lan_credential_gate(
    providers: &crate::ModelProvidersSettings,
    mut route_ids: Vec<String>,
    credential_available: bool,
) -> Vec<String> {
    if route_ids.len() > 1 && !credential_available {
        route_ids.retain(|provider_id| {
            providers
                .providers
                .iter()
                .find(|provider| provider.id() == provider_id)
                .is_none_or(|provider| !matches!(provider, ModelProviderSettings::DynamicLan(_)))
        });
    }
    route_ids
}

pub(crate) fn provider_fallback_allowed(kind: ProviderFailureKind, output_started: bool) -> bool {
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

fn provider_route_fallback_allowed(
    provider: &ModelProviderSettings,
    kind: ProviderFailureKind,
    output_started: bool,
) -> bool {
    provider_fallback_allowed(kind, output_started)
        || (!output_started
            && matches!(provider, ModelProviderSettings::DynamicLan(_))
            && kind == ProviderFailureKind::Authentication)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_retry_reuses_the_failed_input_message() {
        let connection = rusqlite::Connection::open_in_memory().expect("database opens");
        crate::persistence::schema::initialize_database(&connection).expect("database initializes");
        let state = crate::test_support::app_state(connection);
        let first = StartTurnInput {
            run_id: "run-first".to_string(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
            content: "retry this response".to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        prepare_runtime_run(&state, &first).expect("first run prepares");
        let input_message_id: String = state
            .sqlite_writer
            .lock()
            .expect("database lock")
            .query_row(
                "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                [&first.run_id],
                |row| row.get(0),
            )
            .expect("input message reads");
        state
            .sqlite_writer
            .lock()
            .expect("database lock")
            .execute(
                "UPDATE runtime_runs SET status = 'failed' WHERE id = ?1",
                [&first.run_id],
            )
            .expect("first run fails");

        let retry = StartTurnInput {
            run_id: "run-retry".to_string(),
            retry_input_message_id: Some(input_message_id.clone()),
            source_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
            ..first
        };
        prepare_runtime_run(&state, &retry).expect("retry prepares");
        let connection = state.sqlite_writer.lock().expect("database lock");
        let message_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE role = 'user'",
                [],
                |row| row.get(0),
            )
            .expect("message count reads");
        let retry_input: String = connection
            .query_row(
                "SELECT input_message_id FROM runtime_runs WHERE id = 'run-retry'",
                [],
                |row| row.get(0),
            )
            .expect("retry input reads");
        assert_eq!(message_count, 1);
        assert_eq!(retry_input, input_message_id);
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

    #[test]
    fn dynamic_lan_authentication_can_fall_back_before_output_only() {
        let dynamic_lan = crate::test_support::dynamic_lan_provider("dynamic_lan-primary");
        let direct = crate::test_support::provider("direct-primary", "local");
        assert!(provider_route_fallback_allowed(
            &dynamic_lan,
            ProviderFailureKind::Authentication,
            false
        ));
        assert!(!provider_route_fallback_allowed(
            &dynamic_lan,
            ProviderFailureKind::Authentication,
            true
        ));
        assert!(!provider_route_fallback_allowed(
            &direct,
            ProviderFailureKind::Authentication,
            false
        ));
    }

    #[test]
    fn missing_dynamic_lan_credential_prefers_the_configured_direct_route() {
        let providers = crate::ModelProvidersSettings {
            providers: vec![
                crate::test_support::dynamic_lan_provider("dynamic_lan-primary"),
                crate::test_support::provider("direct-fallback", "local"),
            ],
            reasoning_effort: crate::providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
        };
        let configured = vec![
            "dynamic_lan-primary".to_string(),
            "direct-fallback".to_string(),
        ];
        assert_eq!(
            apply_dynamic_lan_credential_gate(&providers, configured.clone(), false),
            ["direct-fallback"]
        );
        assert_eq!(
            apply_dynamic_lan_credential_gate(&providers, configured, true),
            ["dynamic_lan-primary", "direct-fallback"]
        );
    }
}
