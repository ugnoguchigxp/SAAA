//! Conversation provider path. `turns::execute_turn` dispatches here after coding / capability.
#[path = "conversation_context.rs"]
mod conversation_context;
#[path = "conversation_controller/mod.rs"]
mod conversation_controller;
#[path = "conversation_inputs.rs"]
mod conversation_inputs;
#[path = "conversation_prepare.rs"]
mod prepare;
use prepare::{
    compose_after_connect, provider_input_budget, world_free_history, FreshProviderContext,
};
#[path = "role_step_sink.rs"]
mod role_step_sink;
#[path = "conversation_state_answer.rs"]
mod state_answer;
#[path = "conversation_stream.rs"]
mod streaming;

use super::event_hub::RuntimeEventSender;
use crate::ipc_contract::{ConversationMessage, RuntimeEvent};
use crate::providers::routing::{effective_conversation_route_ids, resolve_harness_llm_provider};
use crate::{
    begin_provider_session, finish_dynamic_lan_provider_session, finish_provider_session, memory,
    now_iso, persist_conversation_success_with_state, stream_model_provider,
    stream_voice_aware_dynamic_lan_provider, update_runtime_provider, AppState, CleanupOutcome,
    ModelProviderSettings, ModelStreamContext, ProviderAttemptOutcome, ProviderFailureKind,
    ProviderOutputPersistence, RunCancellation, StartTurnInput, TurnExecutionFailure,
};
use conversation_context::compose_provider_history;
use conversation_controller::execute as execute_reasoning;
use std::sync::Arc;
#[path = "conversation_role_codex.rs"]
mod role_codex;
#[cfg(test)]
use role_codex::role_codex_prompt;
use role_codex::{execute_role_codex_step, CodexStepRequest};

pub(crate) async fn execute_conversation_turn(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    execute_conversation_turn_with_candidates(state, input, on_event, cancellation, Vec::new())
        .await
}

#[derive(Clone)]
struct RoleCandidate {
    step_id: String,
    purpose: String,
    content: String,
}

#[derive(Clone)]
struct ActiveRoleStep {
    step_id: String,
    purpose: String,
}

async fn execute_conversation_turn_with_candidates(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    mut role_candidates: Vec<RoleCandidate>,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    let context_started = std::time::Instant::now();
    let conversation_inputs::Inputs {
        mut providers,
        route,
        security,
        identity,
        regional,
        scope,
        personal_source_error,
        configuration_fingerprint,
        role_dispatch,
        ..
    } = conversation_inputs::load(state, input)?;
    let role_provider_step = matches!(
        &role_dispatch,
        Some(conversation_inputs::conversation_inputs_roles::RoleDispatch::Provider { .. })
    );
    let role_provider_max_input_bytes = match &role_dispatch {
        Some(conversation_inputs::conversation_inputs_roles::RoleDispatch::Provider {
            max_input_bytes,
        }) => Some(*max_input_bytes as usize),
        _ => None,
    };
    let active_provider_step = if role_provider_step {
        state.sqlite_writer.write(|connection| {
            connection
                .query_row(
                    "SELECT id,purpose FROM rr_steps WHERE root_id=?1 AND status='running' ORDER BY ordinal LIMIT 1",
                    [&input.run_id],
                    |row| {
                        Ok(ActiveRoleStep {
                            step_id: row.get(0)?,
                            purpose: row.get(1)?,
                        })
                    },
                )
                .map(Some)
                .map_err(|error| error.to_string())
        })?
    } else {
        None
    };
    crate::providers::http_metrics::record("contextInputsLoad", context_started.elapsed());
    if scope.status != "resolved" {
        crate::runtime::context::generation::record_red(
            state,
            &input.run_id,
            scope.reason_code.as_deref().unwrap_or("scope-invalid"),
        );
        return Err(TurnExecutionFailure::configuration(format!(
            "Context scope could not be resolved: {}",
            scope.reason_code.as_deref().unwrap_or("scope-invalid")
        )));
    }
    if let Some(error) = personal_source_error {
        crate::runtime::context::generation::record_red(
            state,
            &input.run_id,
            "personal-state-source-unavailable",
        );
        return Err(TurnExecutionFailure::configuration(error));
    }
    // Narrow present-state questions have a host-verifiable answer. Persist the card through the
    // normal completion path so the UI and voice completion still share one message, but never
    // ask a provider to turn an unavailable observation into a current-state assertion.
    let state_query = crate::runtime::context::state_answer::is_state_query(&input.content);
    let verified_events = on_event;
    let claim_events =
        crate::runtime::context::world::state_claim::ClaimEvents(on_event.clone_box());
    let on_event: &dyn RuntimeEventSender = if state_query { &claim_events } else { on_event };
    if state_query && (route.source == "harness" || role_dispatch.is_some()) {
        return state_answer::persist_card(state, input, verified_events);
    }
    // Compose only at a concrete provider dispatch boundary. A generic pre-compose would use
    // the wrong provider budget and could reject a request that fits its selected provider.
    crate::providers::http_metrics::record("contextAssemblyTotal", context_started.elapsed());
    if let Some(conversation_inputs::conversation_inputs_roles::RoleDispatch::CodexSdk {
        model,
        max_input_bytes,
        binding,
    }) = role_dispatch
    {
        let binding = binding.ok_or_else(|| {
            TurnExecutionFailure::configuration(
                "Role-routing Codex dispatch has no persisted step binding",
            )
        })?;
        let FreshProviderContext { history, world, .. } = compose_after_connect(
            state,
            input,
            &identity,
            &regional,
            crate::runtime::context::broker::ProviderInputBudget::openai_compatible(),
        )
        .map_err(|error| TurnExecutionFailure::configuration(context_recovery_message(&error)))?;
        let result = execute_role_codex_step(
            state,
            input,
            on_event,
            cancellation.clone(),
            CodexStepRequest {
                step_id: binding.step_id.clone(),
                revision: binding.revision,
                config_fingerprint: binding.config_fingerprint,
                purpose: binding.purpose.clone(),
                model,
                max_input_bytes: max_input_bytes as usize,
                current_request: role_step_request(
                    &binding.purpose,
                    &input.content,
                    &role_candidates,
                )?,
            },
            &history,
            route.timeout_ms,
            world,
        )
        .await?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        let usage_json = result.usage.as_ref().map(|usage| usage.as_json());
        if binding.purpose == "review" {
            let response = parse_review_response(&result.content)?;
            let review_outcome = state.sqlite_writer.write(|connection| {
                crate::role_routing::repository::advance_review_step(
                    connection,
                    &input.run_id,
                    &response,
                    usage_json.as_deref(),
                    now_ms,
                )
            })?;
            let draft = role_candidates
                .iter()
                .find(|candidate| matches!(candidate.purpose.as_str(), "respond" | "reconsider"))
                .cloned()
                .ok_or_else(|| {
                    TurnExecutionFailure::configuration(
                        "Role-routing reviewed response lost its author draft",
                    )
                })?;
            match review_outcome {
                crate::role_routing::repository::ReviewStepOutcome::Revise(decision) => {
                    role_candidates.push(RoleCandidate {
                        step_id: binding.step_id,
                        purpose: "review".into(),
                        content: serde_json::to_string(&decision).map_err(|error| {
                            TurnExecutionFailure::configuration(error.to_string())
                        })?,
                    });
                    return Box::pin(execute_conversation_turn_with_candidates(
                        state,
                        input,
                        on_event,
                        cancellation,
                        role_candidates,
                    ))
                    .await;
                }
                crate::role_routing::repository::ReviewStepOutcome::KeepDraft => {
                    return persist_conversation_success_with_state(
                        state,
                        input,
                        &draft.content,
                        |connection, message| {
                            crate::role_routing::repository::accept_reviewed_draft(
                                connection,
                                &input.run_id,
                                &draft.step_id,
                                &message.id,
                                now_ms,
                            )
                        },
                    )
                    .map_err(Into::into);
                }
            }
        }
        if state.sqlite_writer.write(|connection| {
            crate::role_routing::repository::advance_provider_step_with_usage(
                connection,
                &input.run_id,
                &result.content,
                usage_json.as_deref(),
                now_ms,
            )
        })? {
            role_candidates.push(RoleCandidate {
                step_id: binding.step_id,
                purpose: binding.purpose,
                content: result.content,
            });
            return Box::pin(execute_conversation_turn_with_candidates(
                state,
                input,
                on_event,
                cancellation,
                role_candidates,
            ))
            .await;
        }
        return persist_conversation_success_with_state(
            state,
            input,
            &result.content,
            |connection, message| {
                if let Some(usage_json) = usage_json.as_deref() {
                    crate::role_routing::repository::record_step_usage(
                        connection,
                        &input.run_id,
                        usage_json,
                    )?;
                }
                crate::role_routing::repository::accept_provider_turn(
                    connection,
                    &input.run_id,
                    &message.id,
                    now_ms,
                )
            },
        )
        .map_err(Into::into);
    }
    let shared_larm_voice =
        route.source == "harness" && input.input_origin == "voice" && crate::larm_voice::enabled();
    let harness = providers.harness.clone();
    if let Some(client) =
        crate::providers::reasoning_mcp::for_turn(route.source == "harness", input, &cancellation)
            .await?
    {
        let FreshProviderContext {
            envelope,
            world: world_live,
            history,
        } = compose_after_connect(
            state,
            input,
            &identity,
            &regional,
            crate::runtime::context::broker::ProviderInputBudget::openai_compatible(),
        )
        .map_err(|error| TurnExecutionFailure::configuration(context_recovery_message(&error)))?;
        let reasoning_world_free_history = world_free_history(&history, world_live.as_ref());
        let reasoning_world_free_history =
            reasoning_world_free_history.as_deref().unwrap_or(&history);
        // The MCP request is its own dispatch boundary. It receives World as typed evidence only
        // after a final freshness check; a stale frame is removed from both its body and receipt.
        let include_world = world_live
            .as_ref()
            .is_some_and(|world| world.revalidate_current());
        let manifest_selected: Vec<crate::runtime::context::source::Candidate> = envelope
            .selected
            .iter()
            .filter(|candidate| {
                include_world
                    || candidate.source_kind != crate::runtime::context::world::source::WORLD_KIND
            })
            .cloned()
            .collect();
        let manifest_omitted: Vec<crate::runtime::context::source::Candidate> = envelope
            .omitted
            .iter()
            .filter(|candidate| {
                include_world
                    || candidate.source_kind != crate::runtime::context::world::source::WORLD_KIND
            })
            .cloned()
            .collect();
        let reasoning_history = if include_world {
            &history
        } else {
            reasoning_world_free_history
        };
        return execute_reasoning(
            state,
            input,
            reasoning_history,
            on_event,
            cancellation,
            &client,
            conversation_controller::ContextManifest {
                selected: &manifest_selected,
                omitted: &manifest_omitted,
                health: envelope.health.status.as_str(),
                world: include_world.then_some(world_live.as_ref()).flatten(),
            },
        )
        .await
        .map_err(Into::into);
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(route.timeout_ms);
    let reasoning_effort = providers.reasoning_effort.clone();
    let max_output_tokens = crate::providers::completion::DEFAULT_MAX_OUTPUT_TOKENS;
    let route_ids = effective_conversation_route_ids(&providers, &route, &security);
    if route_ids.is_empty() {
        return Err(TurnExecutionFailure::configuration(
            "Choose a conversation provider in Settings.",
        ));
    }
    let mut failures: Vec<TurnExecutionFailure> = Vec::new();
    let mut context_health_emitted = false;
    let mut context_health_recorded = false;

    let mut route_ids = std::collections::VecDeque::from(route_ids);
    let mut state_retries = 0;
    while let Some(provider_id) = route_ids.pop_front() {
        if cancellation.is_cancelled() {
            return Err(TurnExecutionFailure::provider(
                ProviderFailureKind::Cancelled,
                "Cancelled by user".to_string(),
            ));
        }
        let remaining_ms = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .as_millis() as u64;
        if remaining_ms == 0 {
            return Err(TurnExecutionFailure::provider(
                ProviderFailureKind::Timeout,
                "Conversation reached its total timeout".into(),
            ));
        }
        let attempt_deadline =
            tokio::time::Instant::now()
                + std::time::Duration::from_millis(remaining_ms.min(
                    route.attempt_timeout_ms.unwrap_or(
                        route.timeout_ms / (1 + route.fallback_provider_ids.len()) as u64,
                    ),
                ));
        if route.source == "harness"
            && provider_id == crate::DYNAMIC_LAN_PROVIDER_ID
            && !shared_larm_voice
        {
            let resolution = tokio::time::timeout_at(
                attempt_deadline,
                resolve_harness_llm_provider(&mut providers, remaining_ms, cancellation.clone()),
            )
            .await;
            match resolution {
                Ok(Ok(_)) => {}
                result => {
                    if cancellation.is_cancelled() {
                        return Err(TurnExecutionFailure::provider(
                            ProviderFailureKind::Cancelled,
                            "Cancelled by user".into(),
                        ));
                    }
                    let (kind, message) = match result {
                        Err(_) => (
                            ProviderFailureKind::Timeout,
                            "Harness discovery reached its timeout".to_string(),
                        ),
                        Ok(Err(error)) => {
                            (crate::providers::route_policy::failure_kind(&error), error)
                        }
                        _ => unreachable!(),
                    };
                    let _ = on_event.send(RuntimeEvent::ProviderFailed {
                        run_id: input.run_id.clone(),
                        provider_id: provider_id.clone(),
                        reason: kind.public_message().as_str().to_string(),
                    });
                    let failure = TurnExecutionFailure::provider(kind, message);
                    if !provider_fallback_allowed(kind, false) {
                        return Err(failure);
                    }
                    failures.push(failure);
                    continue;
                }
            }
        }
        let attempt_timeout_ms = attempt_deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .as_millis() as u64;
        if attempt_timeout_ms == 0 {
            failures.push(TurnExecutionFailure::provider(
                ProviderFailureKind::Timeout,
                "Provider setup reached its timeout".into(),
            ));
            continue;
        }
        let Some(provider) = providers
            .providers
            .iter()
            .find(|provider| provider.id() == provider_id && provider.enabled())
            .cloned()
        else {
            failures.push(TurnExecutionFailure::configuration(format!(
                "{provider_id}: provider is disabled or missing"
            )));
            continue;
        };

        update_runtime_provider(state, &input.run_id, provider.id())?;
        let activity_now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        state.sqlite_writer.write(|connection| {
            crate::role_routing::repository::record_actor_activity(
                connection,
                &input.run_id,
                "provider_started",
                activity_now_ms,
            )
            .map(|_| ())
        })?;
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
            finish_provider_session(
                state,
                &session_id,
                "failed",
                Some(ProviderFailureKind::ClientDisconnected),
            )?;
            return Err(TurnExecutionFailure::provider(
                ProviderFailureKind::ClientDisconnected,
                ProviderFailureKind::ClientDisconnected
                    .public_message()
                    .as_str()
                    .to_string(),
            ));
        }
        let budget = match provider_input_budget(state, input, &session_id, &provider) {
            Ok(budget) => budget,
            Err(error) => {
                finish_provider_session(
                    state,
                    &session_id,
                    "failed",
                    Some(ProviderFailureKind::Contract),
                )?;
                return Err(TurnExecutionFailure::configuration(error));
            }
        };
        let FreshProviderContext {
            envelope,
            world: world_live,
            mut history,
        } = match compose_after_connect(state, input, &identity, &regional, budget) {
            Ok(context) => context,
            Err(error) => {
                finish_provider_session(
                    state,
                    &session_id,
                    "failed",
                    Some(ProviderFailureKind::Contract),
                )?;
                return Err(TurnExecutionFailure::configuration(
                    context_recovery_message(&error),
                ));
            }
        };
        if state_query {
            history.insert(
                0,
                crate::ipc_contract::ConversationMessage {
                    id: "host-state-claim-contract".into(),
                    conversation_id: input.conversation_id.clone(),
                    role: "system".into(),
                    content: crate::runtime::context::world::state_claim::INSTRUCTION.into(),
                    parts: None,
                    created_at: now_iso(),
                },
            );
        }
        if let Some(step) = active_provider_step.as_ref() {
            let task = role_step_request(&step.purpose, &input.content, &role_candidates)?;
            if task != input.content {
                let current_index = history
                    .iter()
                    .rposition(|message| {
                        message.role == "user" && message.content.trim() == input.content.trim()
                    })
                    .ok_or_else(|| {
                        TurnExecutionFailure::configuration(
                            "Role-routing task could not bind the persisted input message",
                        )
                    })?;
                // Keep the persisted user input as the final user message. Generation
                // accounting uses that exact message to prove there is one current
                // instruction; the role-specific work is host-owned control context.
                history.insert(
                    current_index,
                    crate::ipc_contract::ConversationMessage {
                        id: format!("host-role-task-{}", step.step_id),
                        conversation_id: input.conversation_id.clone(),
                        role: "system".into(),
                        content: task,
                        parts: None,
                        created_at: now_iso(),
                    },
                );
            }
        }
        if let Some(max_input_bytes) = role_provider_max_input_bytes {
            let encoded_bytes = serde_json::to_vec(&history)
                .map_err(|error| TurnExecutionFailure::configuration(error.to_string()))?
                .len();
            if encoded_bytes > max_input_bytes {
                finish_provider_session(
                    state,
                    &session_id,
                    "failed",
                    Some(ProviderFailureKind::Contract),
                )?;
                return Err(TurnExecutionFailure::configuration(format!(
                    "Role-routing provider input exceeds actor limit ({encoded_bytes} > {max_input_bytes} bytes)"
                )));
            }
        }
        if envelope.health.status == crate::runtime::context::health::Status::Yellow {
            let _ = on_event.send(RuntimeEvent::Activity {
                run_id: input.run_id.clone(),
                kind: "context-degraded".into(),
                summary: format!(
                    "Context was safely reduced ({} source item(s) omitted).",
                    envelope.health.omitted_sources
                ),
            });
        }
        crate::runtime::turn_activity::send_context_window_once(
            &mut context_health_emitted,
            on_event,
            &input.run_id,
            &envelope.context_health,
        );
        if !context_health_recorded && memory::control_plane::memory_enabled() {
            let health = &envelope.context_health;
            let _ = state.sqlite_writer.write(|connection| {
                memory::control_plane::record_projection_event(
                    connection,
                    health.status,
                    health.projected_bytes,
                    health.hard_limit_bytes,
                    health.output_reserve_bytes,
                    health.repair_count,
                    &now_iso(),
                )
            });
            context_health_recorded = true;
        }
        // Role child output is a candidate until the ledger transaction adopts it. Keep stream
        // text inside the step sink so neither the UI nor TTS can observe an uncommitted draft.
        let role_step_sink = role_provider_step.then(|| {
            role_step_sink::BufferedRoleStepSink::new(input.run_id.clone(), on_event.clone_box())
        });
        let provider_events: &dyn RuntimeEventSender = role_step_sink
            .as_ref()
            .map(|sink| sink as &dyn RuntimeEventSender)
            .unwrap_or(on_event);
        let outcome = streaming::attempt(
            &provider,
            &history,
            attempt_timeout_ms,
            ModelStreamContext {
                reasoning_effort: &reasoning_effort,
                max_output_tokens,
                input,
                on_event: provider_events,
                cancellation: cancellation.clone(),
                context_health: envelope.health.status.as_str(),
                context_sources: &envelope.selected,
                context_omissions: &envelope.omitted,
                output_persistence: Some(ProviderOutputPersistence {
                    state,
                    session_id: &session_id,
                    world: world_live.as_ref(),
                }),
            },
            &harness,
            shared_larm_voice,
        )
        .await;
        match outcome {
            ProviderAttemptOutcome::Completed { content, cleanup } => {
                let (content, state_validation) = if state_query {
                    state_answer::accept(
                        state,
                        input,
                        world_live.as_ref(),
                        &content,
                        verified_events,
                    )
                } else {
                    (content, state_answer::Validation::None)
                };
                if matches!(
                    &provider,
                    ModelProviderSettings::DynamicLan(_) | ModelProviderSettings::AgentSession(_)
                ) {
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
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis() as i64)
                    .unwrap_or(0);
                if active_provider_step
                    .as_ref()
                    .is_some_and(|step| step.purpose == "review")
                {
                    let response = parse_review_response(&content)?;
                    let review_outcome = state.sqlite_writer.write(|connection| {
                        crate::role_routing::repository::advance_review_step(
                            connection,
                            &input.run_id,
                            &response,
                            None,
                            now_ms,
                        )
                    })?;
                    let draft = role_candidates
                        .iter()
                        .find(|candidate| {
                            matches!(candidate.purpose.as_str(), "respond" | "reconsider")
                        })
                        .cloned()
                        .ok_or_else(|| {
                            TurnExecutionFailure::configuration(
                                "Role-routing reviewed response lost its author draft",
                            )
                        })?;
                    match review_outcome {
                        crate::role_routing::repository::ReviewStepOutcome::Revise(decision) => {
                            role_candidates.push(RoleCandidate {
                                step_id: active_provider_step
                                    .as_ref()
                                    .expect("review step")
                                    .step_id
                                    .clone(),
                                purpose: "review".into(),
                                content: serde_json::to_string(&decision).map_err(|error| {
                                    TurnExecutionFailure::configuration(error.to_string())
                                })?,
                            });
                            return Box::pin(execute_conversation_turn_with_candidates(
                                state,
                                input,
                                on_event,
                                cancellation,
                                role_candidates,
                            ))
                            .await;
                        }
                        crate::role_routing::repository::ReviewStepOutcome::KeepDraft => {
                            return persist_conversation_success_with_state(
                                state,
                                input,
                                &draft.content,
                                |connection, message| {
                                    crate::role_routing::repository::accept_reviewed_draft(
                                        connection,
                                        &input.run_id,
                                        &draft.step_id,
                                        &message.id,
                                        now_ms,
                                    )
                                },
                            )
                            .map_err(Into::into);
                        }
                    }
                }
                if role_provider_step
                    && state.sqlite_writer.write(|connection| {
                        crate::role_routing::repository::advance_provider_step(
                            connection,
                            &input.run_id,
                            &content,
                            now_ms,
                        )
                    })?
                {
                    // Re-enter through the normal loader so the next dispatch comes from the
                    // newly claimed persisted step. The finite recipe budget bounds recursion.
                    if let Some(step) = active_provider_step.as_ref() {
                        role_candidates.push(RoleCandidate {
                            step_id: step.step_id.clone(),
                            purpose: step.purpose.clone(),
                            content,
                        });
                    }
                    return Box::pin(execute_conversation_turn_with_candidates(
                        state,
                        input,
                        on_event,
                        cancellation,
                        role_candidates,
                    ))
                    .await;
                }
                return persist_conversation_success_with_state(
                    state,
                    input,
                    &content,
                    |connection, message| {
                        match &state_validation {
                            state_answer::Validation::Model => {
                                world_live
                                    .as_ref()
                                    .ok_or("state-claim-unavailable")?
                                    .validate_claim_commit(connection)?;
                            }
                            state_answer::Validation::Host((service, frame)) => {
                                service
                                    .validate_db_result(connection, frame)
                                    .map_err(|error| error.code().to_string())?;
                            }
                            state_answer::Validation::None => {}
                        }
                        crate::role_routing::repository::accept_provider_turn(
                            connection,
                            &input.run_id,
                            &message.id,
                            now_ms,
                        )
                    },
                )
                .map_err(Into::into);
            }
            ProviderAttemptOutcome::Cancelled { cleanup, .. } => {
                if matches!(
                    &provider,
                    ModelProviderSettings::DynamicLan(_) | ModelProviderSettings::AgentSession(_)
                ) {
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
                return Err(TurnExecutionFailure::provider(
                    ProviderFailureKind::Cancelled,
                    "Cancelled by user".to_string(),
                ));
            }
            ProviderAttemptOutcome::Failed {
                kind,
                public_message,
                output_started,
                cleanup,
            } => {
                let reason = public_message.as_str();
                if matches!(
                    &provider,
                    ModelProviderSettings::DynamicLan(_) | ModelProviderSettings::AgentSession(_)
                ) {
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
                if state_query
                    && matches!(
                        kind,
                        ProviderFailureKind::RequiredContextUnavailable
                            | ProviderFailureKind::ContextScopeChanged
                    )
                {
                    if state_retries == 0 {
                        state_retries += 1;
                        route_ids.push_front(provider_id.clone());
                        continue;
                    }
                    return state_answer::persist_card(state, input, verified_events);
                }
                let _ = on_event.send(RuntimeEvent::ProviderFailed {
                    run_id: input.run_id.clone(),
                    provider_id: provider.id().to_string(),
                    reason: reason.to_string(),
                });
                let failure =
                    TurnExecutionFailure::provider(kind, format!("{}: {reason}", provider.id()));
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
        let code = failures
            .last()
            .map(|failure| failure.code)
            .unwrap_or(crate::runtime::contracts::RunFailureCode::ConfigurationError);
        Err(TurnExecutionFailure::unsupervised(
            code,
            format!(
                "Configured provider attempts failed. {}",
                failures
                    .into_iter()
                    .map(|failure| failure.message)
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        ))
    }
}

fn role_step_request(
    purpose: &str,
    original_request: &str,
    candidates: &[RoleCandidate],
) -> Result<String, TurnExecutionFailure> {
    match purpose {
        "review" => {
            let target = candidates
                .iter()
                .rev()
                .find(|candidate| matches!(candidate.purpose.as_str(), "respond" | "reconsider"))
                .ok_or_else(|| {
                    TurnExecutionFailure::configuration(
                        "Role-routing review has no exact author draft",
                    )
                })?;
            Ok(format!(
                "Independently review the draft against the original request. Return only one JSON object matching the supplied schema. Do not rewrite the answer. Every issue evidenceRef must be exactly `rr-output-{}`; use an empty issues array when there is no supported issue.\n\n<original-request>\n{}\n</original-request>\n\n<review-target>\n{}\n</review-target>",
                target.step_id, original_request, target.content
            ))
        }
        "revise" => {
            let target = candidates
                .iter()
                .find(|candidate| matches!(candidate.purpose.as_str(), "respond" | "reconsider"))
                .ok_or_else(|| {
                    TurnExecutionFailure::configuration("Role-routing revision has no author draft")
                })?;
            let review = candidates
                .iter()
                .rev()
                .find(|candidate| candidate.purpose == "review")
                .ok_or_else(|| {
                    TurnExecutionFailure::configuration(
                        "Role-routing revision has no independent review",
                    )
                })?;
            Ok(format!(
                "Revise the draft only where the review identifies supported issues. Preserve correct claims and do not mention the review process.\n\n<original-request>\n{}\n</original-request>\n\n<draft>\n{}\n</draft>\n\n<review>\n{}\n</review>",
                original_request, target.content, review.content
            ))
        }
        "respond" | "reconsider" | "frontend" => Ok(original_request.to_string()),
        _ => Err(TurnExecutionFailure::configuration(format!(
            "Unsupported role-routing step purpose: {purpose}"
        ))),
    }
}

fn parse_review_response(
    content: &str,
) -> Result<crate::role_routing::review::ReviewResponse, TurnExecutionFailure> {
    if content.len() > 64 * 1024 {
        return Err(TurnExecutionFailure::configuration(
            "Role-routing review exceeds its output limit",
        ));
    }
    serde_json::from_str(content).map_err(|_| {
        TurnExecutionFailure::configuration(
            "Role-routing reviewer returned an invalid structured response",
        )
    })
}

#[cfg(test)]
mod role_review_tests {
    use super::*;

    #[test]
    fn rr_24_review_target_is_exact_draft() {
        let candidates = vec![RoleCandidate {
            step_id: "author-step".into(),
            purpose: "respond".into(),
            content: "exact private draft".into(),
        }];
        let prompt =
            role_step_request("review", "original request", &candidates).expect("review prompt");
        assert!(prompt.contains("<review-target>\nexact private draft\n</review-target>"));
        assert!(prompt.contains("rr-output-author-step"));
        assert!(!prompt.contains("model"));
        assert!(!prompt.contains("actor"));
    }

    #[test]
    fn rr_24_provider_review_rejects_unknown_fields() {
        assert!(parse_review_response(r#"{"issues":[],"trusted":true}"#).is_err());
        assert!(parse_review_response(r#"{"issues":[]}"#).is_ok());
    }

    #[test]
    fn rr_25_reviser_receives_host_decision_not_model_instructions() {
        let candidates = vec![
            RoleCandidate {
                step_id: "author-step".into(),
                purpose: "respond".into(),
                content: "draft".into(),
            },
            RoleCandidate {
                step_id: "review-step".into(),
                purpose: "review".into(),
                content: r#"{"revisionAllowed":true,"verifiedIssues":[],"unresolvedIssues":[],"completedRounds":0,"maxRounds":1}"#.into(),
            },
        ];
        let prompt = role_step_request("revise", "request", &candidates).expect("revise prompt");
        assert!(prompt.contains("revisionAllowed"));
        assert!(prompt.contains("<draft>\ndraft\n</draft>"));
    }
}

#[path = "conversation_recovery.rs"]
mod recovery;
pub(crate) use recovery::provider_fallback_allowed;
use recovery::{context_recovery_message, provider_route_fallback_allowed};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_19_codex_prompt_keeps_current_request_and_bounds_history() {
        let history = vec![ConversationMessage {
            parts: None,
            id: "old".into(),
            conversation_id: "c".into(),
            role: "assistant".into(),
            content: "x".repeat(60_000),
            created_at: "1".into(),
        }];
        let prompt =
            role_codex_prompt(&history, "current request", 48 * 1024).expect("bounded prompt");
        assert!(prompt.contains("current request"));
        assert!(prompt.len() < 50_000);
        assert!(prompt.contains("untrusted conversation data"));
    }

    #[test]
    fn rr_19_codex_prompt_rejects_an_unfit_current_request() {
        let error =
            role_codex_prompt(&[], &"x".repeat(512), 128).expect_err("must not trim user request");
        assert!(error.message.contains("input limit"));
    }

    #[test]
    fn world_free_history_removes_the_world_block_for_unsupported_providers() {
        let world = crate::runtime::context::world::turn::WorldLive::for_test(
            true,
            "WITH_WORLD",
            Some("WITHOUT_WORLD"),
        );
        let history = vec![
            ConversationMessage {
                parts: None,
                id: "system".into(),
                conversation_id: "c".into(),
                role: "system".into(),
                content: "policy".into(),
                created_at: "system".into(),
            },
            ConversationMessage {
                parts: None,
                id: "world".into(),
                conversation_id: "c".into(),
                role: "assistant".into(),
                content: "WITH_WORLD".into(),
                created_at: "1".into(),
            },
            ConversationMessage {
                parts: None,
                id: "user".into(),
                conversation_id: "c".into(),
                role: "user".into(),
                content: "hello".into(),
                created_at: "2".into(),
            },
        ];
        let stripped = world_free_history(&history, Some(&world)).expect("world block present");
        assert_eq!(stripped.len(), 3);
        assert_eq!(stripped[1].content, "WITHOUT_WORLD");
        assert_eq!(stripped[2].content, "hello");
        // When the World was the only selected candidate the block disappears entirely.
        let only_world =
            crate::runtime::context::world::turn::WorldLive::for_test(true, "WITH_WORLD", None);
        let stripped =
            world_free_history(&history, Some(&only_world)).expect("world block present");
        assert_eq!(stripped.len(), 2);
        assert_eq!(stripped[0].content, "policy");
        assert_eq!(stripped[1].content, "hello");
        // Without a World there is nothing to strip, so the original borrow is reused.
        assert!(world_free_history(&history, None).is_none());
        assert!(world_free_history(
            &history,
            Some(&crate::runtime::context::world::turn::WorldLive::without_blocks())
        )
        .is_none());
    }

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
            scope_refs: Vec::new(),
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        crate::runtime::turns::prepare_runtime_run(&state, &first).expect("first run prepares");
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
            scope_refs: Vec::new(),
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
            ..first
        };
        crate::runtime::turns::prepare_runtime_run(&state, &retry).expect("retry prepares");
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
            ProviderFailureKind::Capacity,
            ProviderFailureKind::Unavailable,
            ProviderFailureKind::Upstream,
            ProviderFailureKind::Network,
            ProviderFailureKind::Timeout,
            ProviderFailureKind::AllocationLost,
        ] {
            assert!(provider_fallback_allowed(kind, false), "{}", kind.as_str());
            assert!(!provider_fallback_allowed(kind, true), "{}", kind.as_str());
        }
        for kind in [
            ProviderFailureKind::Policy,
            ProviderFailureKind::Authentication,
            ProviderFailureKind::Contract,
            ProviderFailureKind::Protocol,
            ProviderFailureKind::RequestTooLarge,
            ProviderFailureKind::RequiredContextOverflow,
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
    fn authentication_never_switches_providers() {
        let dynamic_lan = crate::test_support::dynamic_lan_provider("dynamic_lan-primary");
        let direct = crate::test_support::provider("direct-primary", "local");
        assert!(!provider_route_fallback_allowed(
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
}
