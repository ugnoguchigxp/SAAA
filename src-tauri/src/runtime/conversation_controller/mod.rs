//! Opt-in voice reasoning path. The existing run remains the persistence owner.
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{
    ipc_contract::{ConversationMessage, RuntimeEvent},
    AppState, RunCancellation, StartTurnInput,
};
use saaa_reasoning_contract::{
    Budget, Constraints, Context, Message, Request, Role, TIMEOUT_MS, VERSION,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
mod classifier;
static REVISION: AtomicU64 = AtomicU64::new(1);

pub(crate) struct ContextManifest<'a> {
    pub(crate) selected: &'a [crate::runtime::context::source::Candidate],
    pub(crate) omitted: &'a [crate::runtime::context::source::Candidate],
    pub(crate) health: &'a str,
    pub(crate) world: Option<&'a crate::runtime::context::world::turn::WorldLive>,
}

pub(crate) async fn execute(
    state: &AppState,
    input: &StartTurnInput,
    history: &[ConversationMessage],
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    client: &crate::providers::reasoning_mcp::Client,
    context: ContextManifest<'_>,
) -> Result<ConversationMessage, String> {
    let result = execute_inner(
        state,
        input,
        history,
        on_event,
        cancellation.clone(),
        client,
        context,
    )
    .await;
    if let Err(error) = &result {
        if !cancellation.is_cancelled() {
            let _ = on_event.send(RuntimeEvent::ProviderFailed {
                run_id: input.run_id.clone(),
                provider_id: "reasoning-mcp".into(),
                reason: crate::redact_runtime_text(error),
            });
        }
    }
    result
}
async fn execute_inner(
    state: &AppState,
    input: &StartTurnInput,
    history: &[ConversationMessage],
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    client: &crate::providers::reasoning_mcp::Client,
    context: ContextManifest<'_>,
) -> Result<ConversationMessage, String> {
    let started = Instant::now();
    crate::update_runtime_provider(state, &input.run_id, "reasoning-mcp")?;
    on_event
        .send(RuntimeEvent::Started {
            run_id: input.run_id.clone(),
            route: "conversation.reasoning".into(),
            provider_id: "reasoning-mcp".into(),
        })
        .map_err(|_| "Reasoning event consumer disconnected")?;
    let mut request = project(input, history)?;
    request.constraints.language = state.sqlite_readers.read(|connection| {
        Ok(crate::persistence::settings::regional_preferences::load(connection)?.language)
    })?;
    if request.constraints.language == "system" {
        request.constraints.language = "auto".into();
    }
    fit_context(&mut request, context.selected)?;
    request.budget.timeout_ms = TIMEOUT_MS
        .saturating_sub(started.elapsed().as_millis() as u64)
        .max(1);
    let request_value = serde_json::to_value(&request)
        .map_err(|_| "Reasoning request could not be encoded".to_string())?;
    crate::runtime::context::generation_inputs::verify_required_wire(
        &request_value,
        context.selected,
    )?;
    let request_payload = serde_json::to_vec(&request_value)
        .map_err(|_| "Reasoning request could not be encoded".to_string())?;
    let generation = crate::runtime::context::generation::begin(
        state,
        crate::runtime::context::generation::BeginGeneration {
            run_id: &input.run_id,
            provider_session_id: None,
            provider_id: Some("reasoning-mcp"),
            purpose: "reasoning",
            request_payload: &request_payload,
            envelope_payload: &request_payload,
            current_instruction_count: 1,
        },
    )?;
    if let Some(world) = context.world {
        world.bind(&generation);
    }
    generation.set_health(context.health)?;
    let required_set =
        crate::runtime::context::required::RequiredContextSet::from_selected(context.selected);
    generation.set_required_receipt(&required_set.digest)?;
    generation.add_input(
        "required-context-set",
        &required_set.digest,
        1,
        &required_set.digest,
        "must",
        "reference",
        true,
        None,
    )?;
    for source in context.selected {
        generation.add_input(
            &source.source_kind,
            &source.source_id,
            source.source_version,
            &source.source_digest,
            source.requirement.as_str(),
            source.placement.as_str(),
            true,
            None,
        )?;
    }
    for source in context.omitted {
        generation.add_input(
            &source.source_kind,
            &source.source_id,
            source.source_version,
            &source.source_digest,
            source.requirement.as_str(),
            source.placement.as_str(),
            false,
            Some("budget-or-policy"),
        )?;
    }
    generation.dispatch()?;
    let shadow = crate::larm_voice::classify_shadow(
        &input.conversation_id,
        &input.content,
        cancellation.clone(),
    );
    let answer = with_ack(
        async {
            let response = client.answer(&request, cancellation.clone()).await?;
            response.validate(&request).map_err(str::to_string)?;
            crate::providers::http_metrics::record(
                "reasoningRequestToValidatedAnswer",
                started.elapsed(),
            );
            Ok::<_, String>(response)
        },
        on_event.acknowledge(
            state,
            &input.run_id,
            &input.conversation_id,
            cancellation.clone(),
        ),
        std::time::Duration::from_millis(250),
    );
    tokio::pin!(answer);
    let response = tokio::select! { biased;
        response = &mut answer => response,
        () = shadow => answer.await,
    };
    crate::runtime::context::generation::finish_result(
        &generation,
        &response,
        cancellation.is_cancelled(),
    )?;
    let response = response?;
    // This lock serializes acceptance with cancellation. A committed answer remains
    // history when the user subsequently stops its playback.
    cancellation
        .with_active(|| crate::persist_conversation_success(state, input, &response.speech_text))
}
async fn with_ack<T>(
    result: impl std::future::Future<Output = T>,
    acknowledgement: impl std::future::Future<Output = ()>,
    delay: std::time::Duration,
) -> T {
    tokio::pin!(result);
    tokio::select! { biased;
        result = &mut result => result,
        _ = tokio::time::sleep(delay) => {
            // Poll inference during the acknowledgement, then serialize playback.
            let (result, ()) = tokio::join!(&mut result, acknowledgement);
            result
        }
    }
}
fn project(input: &StartTurnInput, history: &[ConversationMessage]) -> Result<Request, String> {
    let mut messages: Vec<Message> = history
        .iter()
        .filter_map(|m| {
            let role = match m.role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => return None,
            };
            Some(Message {
                role,
                content: m.content.clone(),
            })
        })
        .collect();
    // Projection's current user message is last. Remove only that occurrence.
    if messages
        .last()
        .is_some_and(|m| matches!(m.role, Role::User) && m.content.trim() == input.content.trim())
    {
        messages.pop();
    }
    let truncated = messages.len() > 32
        || history
            .iter()
            .any(|m| m.role != "user" && m.role != "assistant");
    if messages.len() > 32 {
        messages.drain(..messages.len() - 32);
    }
    let mut request = Request {
        schema_version: VERSION.into(),
        conversation_id: input.conversation_id.clone(),
        turn_id: input.run_id.clone(),
        request_id: crate::new_id("reasoning"),
        context_revision: REVISION.fetch_add(1, Ordering::Relaxed),
        request: input.content.trim().into(),
        context: Context {
            messages,
            evidence: vec![saaa_reasoning_contract::Evidence {
                id: "host_capabilities".into(), source: "SAAA host capability declaration".into(),
                content: "This voice reasoning route cannot execute coding tools or pi jobs. When asked to implement, continue, inspect or cancel a coding job, state that this route does not support the action and direct the user to the normal text conversation. Never claim to have executed an action.".into(),
            }],
            truncated,
        },
        constraints: Constraints {
            language: "ja".into(),
            local_only: true,
            max_speech_chars: 240,
        },
        budget: Budget {
            timeout_ms: TIMEOUT_MS,
        },
    };
    Ok(request)
}
fn fit_context(
    request: &mut Request,
    selected: &[crate::runtime::context::source::Candidate],
) -> Result<(), String> {
    use crate::runtime::context::source::Requirement;

    // `messages` are conversational history, while source snapshots are explicitly marked as
    // evidence. Do not rely on a model inferring a current-state source from the rendered prose.
    // Keep the host capability declaration in slot zero and reserve the remaining bounded slots
    // for required sources first, then the current World frame, then optional context.
    let mut candidates: Vec<_> = selected.iter().collect();
    candidates.sort_by_key(|candidate| match candidate.requirement {
        Requirement::Must => 0_u8,
        _ if candidate.source_kind == crate::runtime::context::world::source::WORLD_KIND => 1,
        Requirement::Should => 2,
        Requirement::May => 3,
    });
    let required_count = candidates
        .iter()
        .filter(|candidate| candidate.requirement == Requirement::Must)
        .count();
    if required_count > 7 {
        return Err("required_context_overflow: reasoning evidence slots exhausted".into());
    }
    for (index, candidate) in candidates.into_iter().take(7).enumerate() {
        request
            .context
            .evidence
            .push(saaa_reasoning_contract::Evidence {
                id: format!("source-{index}"),
                source: format!(
                    "{}:{}@{}",
                    candidate.source_kind, candidate.source_id, candidate.source_version
                ),
                content: candidate.content.clone(),
            });
    }
    while request.validate().is_err() || !request.model_input_fits() {
        let Some(index) = request.context.messages.iter().position(|message| {
            !selected
                .iter()
                .filter(|candidate| candidate.requirement == Requirement::Must)
                .any(|candidate| message.content.contains(&candidate.content))
        }) else {
            return Err(
                "required_context_overflow: reasoning input cannot fit without required context"
                    .into(),
            );
        };
        request.context.messages.remove(index);
        request.context.truncated = true;
    }
    request.validate().map_err(str::to_string)?;
    if !request.model_input_fits() {
        return Err("Reasoning input exceeds the model input budget".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
