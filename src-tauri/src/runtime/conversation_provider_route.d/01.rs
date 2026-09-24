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
    larm_provider: &'static str,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    let shared_larm_voice = should_share_larm_voice_session(
        &route.source,
        route.primary_provider_id.as_deref(),
        &input.input_origin,
        input.source_id.as_deref(),
    );
    let harness = providers.harness.clone();
    if active_provider_step
        .as_ref()
        .is_some_and(|step| step.purpose == "frontend")
    {
        return complete_frontend_step(
            state,
            input,
            on_event,
            cancellation,
            role_candidates,
            active_provider_step,
            shared_larm_voice,
            larm_provider,
        )
        .await;
    }
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
        let fill_wait = input.input_origin == "voice"
            && role_candidates
                .iter()
                .any(|candidate| candidate.purpose == "frontend")
            && active_provider_step
                .as_ref()
                .is_some_and(|step| step.purpose == "respond");
        let stream_context = ModelStreamContext {
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
        };
        let outcome = if fill_wait {
            match wait_for_reasoner(
                &provider,
                &history,
                attempt_timeout_ms,
                stream_context,
                &harness,
                shared_larm_voice,
                larm_provider,
                state,
                on_event,
                &cancellation,
            )
            .await
            {
                ReasonerWait::Finished(outcome) => outcome,
                ReasonerWait::TimedOut(cleanup) => {
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_millis() as i64)
                        .unwrap_or(0);
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
                    return persist_conversation_success_with_state(
                        state,
                        input,
                        "すみません、時間内にお答えできませんでした。",
                        |connection, message| {
                            crate::role_routing::repository::accept_provider_turn_with_status(
                                connection,
                                &input.run_id,
                                &message.id,
                                now_ms,
                                "cancelled",
                            )
                        },
                    )
                    .map_err(Into::into);
                }
            }
        } else {
            streaming::attempt(
                &provider,
                &history,
                attempt_timeout_ms,
                stream_context,
                &harness,
                shared_larm_voice,
                larm_provider,
            )
            .await
        };
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

enum ReasonerWait {
    Finished(ProviderAttemptOutcome),
    TimedOut(crate::CleanupOutcome),
}

const FILLER: &str = "まだ確認しています。";

async fn wait_for_reasoner(
    provider: &ModelProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    harness: &crate::HarnessSettings,
    shared_larm_voice: bool,
    larm_provider: &'static str,
    state: &AppState,
    on_event: &dyn RuntimeEventSender,
    parent: &Arc<RunCancellation>,
) -> ReasonerWait {
    let step_cancel = Arc::new(RunCancellation::default());
    let ModelStreamContext {
        reasoning_effort,
        max_output_tokens,
        input,
        on_event: provider_events,
        context_health,
        context_sources,
        context_omissions,
        output_persistence,
        ..
    } = context;
    let attempt = streaming::attempt(
        provider,
        history,
        timeout_ms,
        ModelStreamContext {
            reasoning_effort,
            max_output_tokens,
            input,
            on_event: provider_events,
            cancellation: step_cancel.clone(),
            context_health,
            context_sources,
            context_omissions,
            output_persistence,
        },
        harness,
        shared_larm_voice,
        larm_provider,
    );
    tokio::pin!(attempt);
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;
    let mut tick = 0u32;
    let mut deferred = false;
    loop {
        let mut speak = false;
        tokio::select! {
            biased;
            _ = parent.cancelled() => {
                step_cancel.cancel();
                return ReasonerWait::Finished(attempt.await);
            }
            outcome = &mut attempt => return ReasonerWait::Finished(outcome),
            _ = interval.tick() => {
                tick += 1;
                #[cfg(test)]
                crate::runtime::event_hub::reasoning_ack::observe_filler_tick(&input.run_id, tick);
                if tick >= 20 {
                    step_cancel.cancel();
                    let cleanup = match attempt.await {
                        ProviderAttemptOutcome::Completed { cleanup, .. }
                        | ProviderAttemptOutcome::Cancelled { cleanup, .. }
                        | ProviderAttemptOutcome::Failed { cleanup, .. } => cleanup,
                    };
                    return ReasonerWait::TimedOut(cleanup);
                }
                let playing =
                    crate::runtime::event_hub::reasoning_ack::speech_still_playing(&input.run_id);
                let (due, next_deferred) =
                    crate::role_routing::frontend::filler_decision(tick, playing, deferred);
                deferred = next_deferred;
                speak = due;
                if speak {
                    #[cfg(test)]
                    crate::role_routing::frontend::record_filler_tick(tick);
                }
            }
        }
        if speak {
            tokio::select! {
                biased;
                _ = parent.cancelled() => {
                    step_cancel.cancel();
                    return ReasonerWait::Finished(attempt.await);
                }
                outcome = &mut attempt => return ReasonerWait::Finished(outcome),
                _ = on_event.acknowledge_hold(
                    state,
                    &input.run_id,
                    &input.conversation_id,
                    FILLER.to_string(),
                    parent.clone(),
                ) => {}
            }
        }
    }
}

async fn complete_frontend_step(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    mut role_candidates: Vec<RoleCandidate>,
    active_provider_step: Option<&ActiveRoleStep>,
    shared_larm_voice: bool,
    larm_provider: &'static str,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    let (content, ack) = if shared_larm_voice {
        frontend_model_output(state, input, &cancellation, larm_provider).await
    } else {
        (String::new(), None)
    };
    let turn = continue_after_frontend(
        state,
        input,
        on_event,
        cancellation.clone(),
        role_candidates,
        active_provider_step,
        content,
    );
    if let Some(text) = ack {
        let (_, result) = tokio::join!(
            on_event.acknowledge_text(
                state,
                &input.run_id,
                &input.conversation_id,
                text,
                cancellation,
            ),
            turn,
        );
        return result;
    }
    turn.await
}

async fn continue_after_frontend(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    mut role_candidates: Vec<RoleCandidate>,
    active_provider_step: Option<&ActiveRoleStep>,
    content: String,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    if state.sqlite_writer.write(|connection| {
        crate::role_routing::repository::advance_provider_step(
            connection,
            &input.run_id,
            &content,
            now_ms,
        )
    })? {
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
    persist_conversation_success_with_state(state, input, &content, |connection, message| {
        crate::role_routing::repository::accept_provider_turn(
            connection,
            &input.run_id,
            &message.id,
            now_ms,
        )
    })
    .map_err(Into::into)
}

async fn frontend_model_output(
    state: &AppState,
    input: &StartTurnInput,
    cancellation: &Arc<RunCancellation>,
    larm_provider: &'static str,
) -> (String, Option<String>) {
    let timeout_ms = state
        .sqlite_readers
        .read(|connection| {
            Ok(crate::persistence::load_role_routing_settings(connection)?
                .limits
                .frontend_timeout_ms)
        })
        .unwrap_or(1_200);
    let max_ack_chars = state
        .sqlite_readers
        .read(|connection| {
            Ok(crate::persistence::load_role_routing_settings(connection)?
                .speech
                .max_ack_chars)
        })
        .unwrap_or(80);
    let settings = match state
        .sqlite_readers
        .read(|connection| Ok(crate::persistence::load_model_providers(connection)?.harness))
    {
        Ok(settings) => settings,
        Err(_) => return (String::new(), None),
    };
    if cancellation.is_cancelled() {
        return (String::new(), None);
    }
    let call = frontend_completion(input, &settings, larm_provider, timeout_ms, cancellation);
    let raw = match tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), call).await {
        Ok(Ok(raw)) => raw,
        _ => return (String::new(), None),
    };
    let Ok(parsed) = crate::role_routing::frontend::parse(&raw) else {
        return (String::new(), None);
    };
    let ack = crate::role_routing::frontend::ack_text(parsed.ack, false)
        .filter(|text| text.chars().count() <= usize::from(max_ack_chars))
        .map(str::to_string);
    let recorded = serde_json::json!({
        "ack": match parsed.ack {
            crate::role_routing::frontend::Ack::None => "none",
            crate::role_routing::frontend::Ack::Nod => "nod",
            crate::role_routing::frontend::Ack::Greeting => "greeting",
            crate::role_routing::frontend::Ack::Thanks => "thanks",
            crate::role_routing::frontend::Ack::Working => "working",
        },
        "intent": match parsed.intent {
            crate::role_routing::frontend::Intent::Social => "social",
            crate::role_routing::frontend::Intent::Acknowledgement => "acknowledgement",
            crate::role_routing::frontend::Intent::Request => "request",
            crate::role_routing::frontend::Intent::Unclear => "unclear",
        },
        "resolvesTurn": parsed.resolves_turn,
        "confidence": if parsed.high_confidence { "high" } else { "low" },
    })
    .to_string();
    (recorded, ack)
}

async fn frontend_completion(
    input: &StartTurnInput,
    settings: &crate::HarnessSettings,
    larm_provider: &'static str,
    timeout_ms: u64,
    cancellation: &Arc<RunCancellation>,
) -> Result<String, ()> {
    let ready = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(()),
        result = crate::larm_voice::current_at(&input.conversation_id, settings) => result.map_err(|_| ())?,
    };
    let lease = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(()),
        result = ready.session.acquire(larm_provider) => result.map_err(|_| ())?,
    };
    let provider = lease.provider();
    let url = provider.endpoint("chat/completions").map_err(|_| ())?;
    let body = serde_json::json!({
        "model": provider.model,
        "stream": true,
        "max_tokens": 64,
        "temperature": 0.0,
        "chat_template_kwargs": {"enable_thinking": false},
        "response_format": crate::role_routing::frontend::response_format(),
        "messages": [
            {"role": "system", "content": crate::role_routing::frontend::INSTRUCTION},
            {"role": "user", "content": input.content}
        ]
    });
    let client = reqwest::Client::new();
    let token = provider.token().to_string();
    let first = client
        .post(url.clone())
        .bearer_auth(&token)
        .timeout(std::time::Duration::from_millis(timeout_ms.max(1)))
        .json(&body)
        .send()
        .await
        .map_err(|_| ())?;
    let mut response = if matches!(first.status(), reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNPROCESSABLE_ENTITY)
    {
        let mut retry = body;
        retry.as_object_mut().map(|object| object.remove("response_format"));
        client
            .post(url)
            .bearer_auth(token)
            .timeout(std::time::Duration::from_millis(timeout_ms.max(1)))
            .json(&retry)
            .send()
            .await
            .map_err(|_| ())?
    } else {
        first
    };
    if !response.status().is_success() {
        return Err(());
    }
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= 65_536 {
            break;
        }
        let Some(chunk) = response.chunk().await.map_err(|_| ())? else {
            break;
        };
        let room = 65_536 - bytes.len();
        let take = chunk.len().min(room);
        bytes.extend_from_slice(&chunk[..take]);
    }
    Ok(collect_completion_text(&bytes))
}

fn collect_completion_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(content) = value["choices"][0]["message"]["content"].as_str() {
            return content.to_string();
        }
    }
    let mut collected = String::new();
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
            if let Some(piece) = value["choices"][0]["delta"]["content"].as_str() {
                collected.push_str(piece);
            }
        }
    }
    collected
}
