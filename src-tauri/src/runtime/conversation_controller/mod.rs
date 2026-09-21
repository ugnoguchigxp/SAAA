//! Opt-in voice reasoning path. The existing run remains the persistence owner.
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{
    ipc_contract::{ConversationMessage, RuntimeEvent},
    AppState, RunCancellation, StartTurnInput,
};
use saaa_reasoning_contract::TIMEOUT_MS;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
mod classifier;
mod payload;
#[cfg(test)]
use payload::{fit_context, project};
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
    let (mut request, selected, omitted) = payload::prepare(state, input, history, &context)?;
    request.budget.timeout_ms = TIMEOUT_MS
        .saturating_sub(started.elapsed().as_millis() as u64)
        .max(1);
    let request_value = serde_json::to_value(&request)
        .map_err(|_| "Reasoning request could not be encoded".to_string())?;
    crate::runtime::context::generation_inputs::verify_required_wire(&request_value, &selected)?;
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
    if selected
        .iter()
        .any(|source| source.source_kind == crate::runtime::context::world::source::WORLD_KIND)
    {
        if let Some(world) = context.world {
            world.bind(&generation);
        }
    }
    generation.set_health(context.health)?;
    let required_set =
        crate::runtime::context::required::RequiredContextSet::from_selected(&selected);
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
    for source in &selected {
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
    for source in &omitted {
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

mod tests;

mod world_eval_tests;
