pub(super) async fn execute(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    mut role_candidates: Vec<RoleCandidate>,
    providers: &mut crate::ModelProvidersSettings,
    route: &crate::ConversationRouteSettings,
    security: &crate::SecurityRuntimeSettings,
    identity: &crate::CodexAgentRuntimeSettings,
    regional: &crate::persistence::settings::regional_preferences::RegionalPreferences,
    configuration_fingerprint: &str,
    state_query: bool,
    verified_events: &dyn RuntimeEventSender,
    active_provider_step: Option<&ActiveRoleStep>,
    role_provider_step: bool,
    role_provider_max_input_bytes: Option<usize>,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    let shared_larm_voice = should_share_larm_voice_session(
        &route.source,
        route.primary_provider_id.as_deref(),
        &input.input_origin,
        input.source_id.as_deref(),
    );
    let harness = providers.harness.clone();
    if let Some(message) = dispatch_reasoning_client(
        state,
        input,
        on_event,
        cancellation.clone(),
        identity,
        regional,
        route.source == "harness",
    )
    .await?
    {
        return Ok(message);
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
        match prepare_harness_provider(
            input,
            on_event,
            &cancellation,
            providers,
            route,
            &provider_id,
            shared_larm_voice,
            attempt_deadline,
            remaining_ms,
        )
        .await?
        {
            HarnessPreparation::Ready => {}
            HarnessPreparation::Skip(failure) => {
                failures.push(failure);
                continue;
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
        if let Some(step) = active_provider_step {
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
                let content = if let Some(step) = active_provider_step
                    .as_ref()
                    .filter(|step| step.purpose == "tool_specialist")
                {
                    execute_specialist_request(
                        state,
                        input,
                        cancellation.as_ref(),
                        &step.step_id,
                        step.revision,
                        &step.config_fingerprint,
                        &content,
                    )
                    .await?
                } else {
                    content
                };
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
                            return Box::pin(super::execute_conversation_turn_with_candidates(
                                state,
                                input,
                                on_event,
                                cancellation,
                                role_candidates,
                            ))
                            .await;
                        }
                        crate::role_routing::repository::ReviewStepOutcome::AwaitPremium(
                            proposal,
                        ) => {
                            if await_premium_step(state, input, cancellation.clone(), &proposal)
                                .await?
                            {
                                return Box::pin(super::execute_conversation_turn_with_candidates(
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
                    if let Some(step) = active_provider_step {
                        role_candidates.push(RoleCandidate {
                            step_id: step.step_id.clone(),
                            purpose: step.purpose.clone(),
                            content,
                        });
                    }
                    return Box::pin(super::execute_conversation_turn_with_candidates(
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
                            state_answer::Validation::Host(prepared) => {
                                let (service, frame) = prepared.as_ref();
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
    collapse_provider_failures(failures)
}
