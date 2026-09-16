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

pub(crate) async fn execute(
    state: &AppState,
    input: &StartTurnInput,
    history: &[ConversationMessage],
    on_event: &dyn RuntimeEventSender,
    cancellation: Arc<RunCancellation>,
    client: &crate::providers::reasoning_mcp::Client,
) -> Result<ConversationMessage, String> {
    let result = execute_inner(
        state,
        input,
        history,
        on_event,
        cancellation.clone(),
        client,
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
    fit_context(&mut request)?;
    request.budget.timeout_ms = TIMEOUT_MS
        .saturating_sub(started.elapsed().as_millis() as u64)
        .max(1);
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
    }?;
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
    fit_context(&mut request)?;
    Ok(request)
}
fn fit_context(request: &mut Request) -> Result<(), String> {
    while (request.validate().is_err() || !request.model_input_fits())
        && !request.context.messages.is_empty()
    {
        request.context.messages.remove(0);
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
