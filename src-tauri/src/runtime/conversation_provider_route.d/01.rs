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
    let route_budget_ms = if active_provider_step.is_some() {
        let root_deadline = state.sqlite_readers.read(|connection| {
            connection.query_row(
                "SELECT deadline_at_ms FROM rr_roots WHERE root_id=?1",
                [&input.run_id],
                |row| row.get::<_, Option<i64>>(0),
            ).optional().map_err(|error| error.to_string())
        })?;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(i64::MAX);
        root_deadline.flatten().map(|value| value.saturating_sub(now_ms).max(0) as u64)
            .unwrap_or(route.timeout_ms).min(route.timeout_ms)
    } else {
        route.timeout_ms
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(route_budget_ms);
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
        let larm_reasoner_step = larm_provider == "llm"
            && active_provider_step.is_some_and(|step| step.purpose == "respond");
        if larm_reasoner_step {
            record_role_stage(state, input, "ornith-context-started", "start", None, None, None);
        }
        let FreshProviderContext {
            envelope,
            world: world_live,
            mut history,
        } = match compose_after_connect(state, input, &identity, &regional, budget) {
            Ok(context) => context,
            Err(error) => {
                if larm_reasoner_step {
                    record_role_stage(state, input, "ornith-context-finished", "terminal", Some("failure"), Some("context-unavailable"), None);
                }
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
        if larm_reasoner_step {
            record_role_stage(state, input, "ornith-context-finished", "terminal", Some("success"), None, None);
        }
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
                if larm_reasoner_step && kind == ProviderFailureKind::PreparationDeferred {
                    record_role_stage(state, input, "ornith-handoff-finished", "terminal", Some("degraded"), Some("preparation-deferred"), None);
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|duration| duration.as_millis() as i64)
                        .unwrap_or(0);
                    return persist_conversation_success_with_state(
                        state,
                        input,
                        "回答用の接続を準備中です。少し後に、内容を確認してもう一度お試しください。",
                        |connection, message| {
                            crate::role_routing::repository::accept_provider_turn_with_status(
                                connection, &input.run_id, &message.id, now_ms, "cancelled",
                            )
                        },
                    ).map_err(Into::into);
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

async fn wait_for_reasoner(
    provider: &ModelProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    harness: &crate::HarnessSettings,
    shared_larm_voice: bool,
    larm_provider: &'static str,
    _state: &AppState,
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
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;
    loop {
        tokio::select! {
            biased;
            _ = parent.cancelled() => {
                step_cancel.cancel();
                let cleanup = match tokio::time::timeout(std::time::Duration::from_secs(5), &mut attempt).await {
                    Ok(ProviderAttemptOutcome::Completed { cleanup, .. })
                    | Ok(ProviderAttemptOutcome::Cancelled { cleanup, .. })
                    | Ok(ProviderAttemptOutcome::Failed { cleanup, .. }) => cleanup,
                    Err(_) => crate::CleanupOutcome::NotApplicable,
                };
                return ReasonerWait::Finished(ProviderAttemptOutcome::Cancelled {
                    output_started: false,
                    cleanup,
                });
            }
            _ = tokio::time::sleep_until(deadline) => {
                step_cancel.cancel();
                let cleanup = match tokio::time::timeout(std::time::Duration::from_secs(5), &mut attempt).await {
                    Ok(ProviderAttemptOutcome::Completed { cleanup, .. })
                    | Ok(ProviderAttemptOutcome::Cancelled { cleanup, .. })
                    | Ok(ProviderAttemptOutcome::Failed { cleanup, .. }) => cleanup,
                    Err(_) => crate::CleanupOutcome::NotApplicable,
                };
                return ReasonerWait::TimedOut(cleanup);
            }
            _ = interval.tick() => {
                let _ = on_event.send(RuntimeEvent::Activity {
                    run_id: input.run_id.clone(),
                    kind: "reasoner-wait".into(),
                    summary: "回答を準備しています。".into(),
                });
            }
            outcome = &mut attempt => return ReasonerWait::Finished(outcome),
        }
    }
}

async fn complete_frontend_step(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    role_candidates: Vec<RoleCandidate>,
    active_provider_step: Option<&ActiveRoleStep>,
    shared_larm_voice: bool,
    larm_provider: &'static str,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    let _ = on_event.send(crate::ipc_contract::RuntimeEvent::Started {
        run_id: input.run_id.clone(),
        route: "conversation.respond".to_string(),
        provider_id: frontend_provider_id(state),
    });
    let (content, ack) = if shared_larm_voice {
        frontend_model_output(state, input, &cancellation, larm_provider)
            .await
            .map_err(|failure| {
                TurnExecutionFailure::provider(failure.kind, failure.code.to_string())
            })?
    } else {
        (String::new(), None)
    };
    let committed_ack = match ack.as_deref() {
        Some(text) => {
            let message = state.sqlite_writer.write(|connection| {
                crate::runtime::butler_loop::commit_preface(
                    connection,
                    &input.conversation_id,
                    &input.run_id,
                    text,
                )
                .map_err(|error| error.to_string())
            })?;
            message.map(|message| {
                let _ = on_event.send(crate::ipc_contract::RuntimeEvent::MessageCommitted {
                    run_id: input.run_id.clone(),
                    message: message.clone(),
                });
                message
            })
        }
        None => None,
    };
    let turn = continue_after_frontend(
        state,
        input,
        on_event,
        cancellation.clone(),
        role_candidates,
        active_provider_step,
        committed_ack,
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

fn frontend_provider_id(state: &AppState) -> String {
    state
        .sqlite_readers
        .read(|connection| {
            let settings = crate::persistence::load_role_routing_settings(connection)?;
            let actor_id = settings.roles.frontend.unwrap_or_default();
            Ok(settings
                .actors
                .into_iter()
                .find(|actor| actor.id == actor_id)
                .and_then(|actor| actor.provider_id)
                .unwrap_or_else(|| "larm-frontdesk".to_string()))
        })
        .unwrap_or_else(|_| "larm-frontdesk".to_string())
}

fn host_reply_without_reasoner(content: &str) -> Option<String> {
    let parsed = crate::role_routing::frontend::parse(content).ok()?;
    if !crate::role_routing::frontend::resolves_without_reasoner(&parsed) {
        return None;
    }
    crate::role_routing::frontend::spoken_line(&parsed)
}

async fn continue_after_frontend(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    mut role_candidates: Vec<RoleCandidate>,
    active_provider_step: Option<&ActiveRoleStep>,
    committed_ack: Option<ConversationMessage>,
    content: String,
) -> Result<ConversationMessage, TurnExecutionFailure> {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    if let Some(text) = host_reply_without_reasoner(&content) {
        if let Some(message) = committed_ack {
            crate::providers::session_store::seal_committed_assistant(
                state,
                input,
                &message,
                |connection, message| {
                    crate::role_routing::repository::accept_resolved_frontend(
                        connection,
                        &input.run_id,
                        &message.id,
                        now_ms,
                    )
                },
            )
            .map_err(TurnExecutionFailure::from)?;
            return Ok(message);
        }
        return persist_conversation_success_with_state(
            state,
            input,
            &text,
            |connection, message| {
                crate::role_routing::repository::accept_resolved_frontend(
                    connection,
                    &input.run_id,
                    &message.id,
                    now_ms,
                )
            },
        )
        .map_err(Into::into);
    }
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
) -> Result<(String, Option<String>), FrontendStepFailure> {
    let (mut timeout_ms, max_ack_chars) = state
        .sqlite_readers
        .read(|connection| {
            let roles = crate::persistence::load_role_routing_settings(connection)?;
            Ok((
                roles.limits.frontend_timeout_ms,
                roles.speech.max_ack_chars,
            ))
        })
        .unwrap_or((1_200, 80));
    let root_deadline = state.sqlite_readers.read(|connection| {
        connection.query_row(
            "SELECT deadline_at_ms FROM rr_roots WHERE root_id=?1",
            [&input.run_id],
            |row| row.get::<_, Option<i64>>(0),
        ).optional().map_err(|error| error.to_string())
    }).map_err(|_| FrontendStepFailure::new("qwen-root-unavailable", ProviderFailureKind::Contract))?;
    if let Some(root_deadline) = root_deadline.flatten() {
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64).unwrap_or(i64::MAX);
        timeout_ms = timeout_ms.min(root_deadline.saturating_sub(now_ms).max(0) as u64);
    }
    if timeout_ms == 0 {
        return Err(FrontendStepFailure::new("qwen-root-timeout", ProviderFailureKind::Timeout));
    }
    let settings = match state
        .sqlite_readers
        .read(|connection| Ok(crate::persistence::load_model_providers(connection)?.harness))
    {
        Ok(settings) => settings,
        Err(_) => return Err(FrontendStepFailure::new("qwen-settings-unavailable", ProviderFailureKind::Contract)),
    };
    if cancellation.is_cancelled() {
        return Err(FrontendStepFailure::new("qwen-cancelled", ProviderFailureKind::Cancelled));
    }
    if larm_provider != "backchannel" {
        return Err(FrontendStepFailure::new("qwen-route-invalid", ProviderFailureKind::Contract));
    }
    record_role_stage(state, input, "qwen-first-response-started", "start", None, None, None);
    // The shared Agent Connection waits for Ornith and other profile resources. Qwen's
    // first reply uses LARM's independently advertised HTTP model route.
    let call = frontend_completion(state, input, &settings);
    let raw: Result<String, FrontendStepFailure> = tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(FrontendStepFailure::new("qwen-cancelled", ProviderFailureKind::Cancelled)),
        result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), call) => {
            result.unwrap_or_else(|_| Err(FrontendStepFailure::new("qwen-timeout", ProviderFailureKind::Timeout)))
        },
    };
    let raw = match raw {
        Ok(raw) => raw,
        Err(failure) => {
            record_role_stage(state, input, "qwen-first-response-finished", "terminal", Some("failure"), Some(failure.code), None);
            return Err(failure);
        }
    };
    let parsed = match crate::role_routing::frontend::parse(&raw) {
        Ok(parsed) => parsed,
        Err(_) => {
            record_role_stage(state, input, "qwen-first-response-finished", "terminal", Some("failure"), Some("qwen-response-invalid"), None);
            return Err(FrontendStepFailure::new("qwen-response-invalid", ProviderFailureKind::Protocol));
        }
    };
    let parsed = crate::role_routing::frontend::guard_for_input(parsed, &input.content);
    let ack = crate::role_routing::frontend::spoken_line(&parsed)
        .filter(|text| text.chars().count() <= usize::from(max_ack_chars));
    if ack.is_none() {
        record_role_stage(state, input, "qwen-first-response-finished", "terminal", Some("failure"), Some("qwen-reply-invalid"), None);
        return Err(FrontendStepFailure::new("qwen-reply-invalid", ProviderFailureKind::Protocol));
    }
    let recorded = serde_json::json!({
        "kind": parsed.kind,
        "reply": ack.clone().unwrap_or_default(),
    })
    .to_string();
    let decision = match parsed.kind {
        crate::role_routing::frontend::FrontendKind::Greeting => "greeting",
        crate::role_routing::frontend::FrontendKind::Thanks => "thanks",
        crate::role_routing::frontend::FrontendKind::Nod => "nod",
        crate::role_routing::frontend::FrontendKind::Answer => "answer",
        crate::role_routing::frontend::FrontendKind::Handoff => "handoff",
    };
    record_role_stage(state, input, "qwen-first-response-finished", "terminal", Some("success"), None, Some(decision));
    Ok((recorded, ack))
}

#[derive(Debug, Clone, Copy)]
struct FrontendStepFailure {
    code: &'static str,
    kind: ProviderFailureKind,
}

impl FrontendStepFailure {
    fn new(code: &'static str, kind: ProviderFailureKind) -> Self {
        Self { code, kind }
    }
}

fn record_role_stage(
    state: &AppState,
    input: &StartTurnInput,
    event_name: &str,
    phase: &str,
    outcome: Option<&str>,
    failure_code: Option<&str>,
    result_code: Option<&str>,
) {
    let mut attributes = std::collections::BTreeMap::new();
    if let Some(result_code) = result_code {
        attributes.insert("resultCode".into(), crate::persistence::audit::AuditAttributeValue::Tag(result_code.into()));
    }
    let _ = crate::persistence::audit::record_frontend_event(
        state,
        &crate::persistence::audit::FrontendAuditEventInput {
            component: "provider".into(),
            event_name: event_name.into(),
            phase: phase.into(),
            outcome: outcome.map(str::to_string),
            correlation_id: Some(input.run_id.clone()),
            causation_id: None,
            conversation_id: Some(input.conversation_id.clone()),
            runtime_run_id: Some(input.run_id.clone()),
            session_id: None,
            subject_id: Some(input.run_id.clone()),
            failure_code: failure_code.map(str::to_string),
            attributes,
        },
    );
}

async fn frontend_completion(
    state: &AppState,
    input: &StartTurnInput,
    settings: &crate::HarnessSettings,
) -> Result<String, FrontendStepFailure> {
    let mut url = url::Url::parse(&settings.address)
        .map_err(|_| FrontendStepFailure::new("qwen-url-invalid", ProviderFailureKind::Contract))?;
    if !saaa_larm_session::local_url(&url) || url.path() != "/" {
        return Err(FrontendStepFailure::new("qwen-url-invalid", ProviderFailureKind::Contract));
    }
    url.set_path("/v1/chat/completions");
    #[cfg(not(test))]
    let token = zeroize::Zeroizing::new(
        crate::providers::dynamic_lan::credential::load()
            .map_err(|_| FrontendStepFailure::new("qwen-credential-unavailable", ProviderFailureKind::Authentication))?
            .token()
            .to_string(),
    );
    #[cfg(test)]
    let token = zeroize::Zeroizing::new("test-control-token".to_string());
    let body = serde_json::json!({
        "model": "backchannel-qwen35-2b",
        "stream": true,
        "max_tokens": 128,
        "temperature": 0.0,
        "chat_template_kwargs": {"enable_thinking": false},
        "response_format": crate::role_routing::frontend::response_format(),
        "messages": [
            {"role": "system", "content": crate::role_routing::frontend::INSTRUCTION},
            {"role": "user", "content": input.content}
        ]
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|_| FrontendStepFailure::new("qwen-client-invalid", ProviderFailureKind::Internal))?;
    record_role_stage(state, input, "qwen-http-sending", "request", None, None, None);
    let first = client
        .post(url.clone())
        .bearer_auth(token.as_str())
        .json(&body)
        .send()
        .await
        .map_err(|_| FrontendStepFailure::new("qwen-transport-failed", ProviderFailureKind::Network))?;
    record_role_stage(state, input, "qwen-http-headers", "progress", None, None, None);
    let mut response = if matches!(first.status(), reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNPROCESSABLE_ENTITY)
    {
        let mut retry = body;
        retry.as_object_mut().map(|object| object.remove("response_format"));
        client
            .post(url)
            .bearer_auth(token.as_str())
            .json(&retry)
            .send()
            .await
            .map_err(|_| FrontendStepFailure::new("qwen-transport-failed", ProviderFailureKind::Network))?
    } else {
        first
    };
    if !response.status().is_success() {
        return Err(FrontendStepFailure::new("qwen-http-failed", ProviderFailureKind::Upstream));
    }
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= 65_536 {
            break;
        }
        let Some(chunk) = response.chunk().await.map_err(|_| FrontendStepFailure::new("qwen-stream-failed", ProviderFailureKind::ResponseInterrupted))? else {
            break;
        };
        let room = 65_536 - bytes.len();
        let take = chunk.len().min(room);
        bytes.extend_from_slice(&chunk[..take]);
    }
    let content = collect_completion_text(&bytes);
    if content.trim().is_empty() {
        return Err(FrontendStepFailure::new("qwen-empty-response", ProviderFailureKind::Protocol));
    }
    Ok(content)
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
