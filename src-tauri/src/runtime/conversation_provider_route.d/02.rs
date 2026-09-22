async fn dispatch_reasoning_client(
    state: &AppState,
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    identity: &crate::CodexAgentRuntimeSettings,
    regional: &crate::persistence::settings::regional_preferences::RegionalPreferences,
    route_source_is_harness: bool,
) -> Result<Option<ConversationMessage>, TurnExecutionFailure> {
if let Some(client) =
    crate::providers::reasoning_mcp::for_turn(route_source_is_harness, input, &cancellation)
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
    let message = execute_reasoning(
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
    .map_err(TurnExecutionFailure::from)?;
    return Ok(Some(message));
    }
    Ok(None)
}

enum HarnessPreparation {
    Ready,
    Skip(TurnExecutionFailure),
}

async fn prepare_harness_provider(
    input: &StartTurnInput,
    on_event: &dyn RuntimeEventSender,
    cancellation: &Arc<RunCancellation>,
    providers: &mut crate::ModelProvidersSettings,
    route: &crate::ConversationRouteSettings,
    provider_id: &str,
    shared_larm_voice: bool,
    attempt_deadline: tokio::time::Instant,
    remaining_ms: u64,
) -> Result<HarnessPreparation, TurnExecutionFailure> {
    if route.source != "harness"
        || provider_id != crate::DYNAMIC_LAN_PROVIDER_ID
        || shared_larm_voice
    {
        return Ok(HarnessPreparation::Ready);
    }
    let resolution = tokio::time::timeout_at(
        attempt_deadline,
        resolve_harness_llm_provider(providers, remaining_ms, cancellation.clone()),
    )
    .await;
    match resolution {
        Ok(Ok(_)) => Ok(HarnessPreparation::Ready),
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
                Ok(Err(error)) => (crate::providers::route_policy::failure_kind(&error), error),
                _ => unreachable!(),
            };
            let _ = on_event.send(RuntimeEvent::ProviderFailed {
                run_id: input.run_id.clone(),
                provider_id: provider_id.to_string(),
                reason: kind.public_message().as_str().to_string(),
            });
            let failure = TurnExecutionFailure::provider(kind, message);
            if !provider_fallback_allowed(kind, false) {
                return Err(failure);
            }
            Ok(HarnessPreparation::Skip(failure))
        }
    }
}

fn collapse_provider_failures(
    mut failures: Vec<TurnExecutionFailure>,
) -> Result<ConversationMessage, TurnExecutionFailure> {
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
