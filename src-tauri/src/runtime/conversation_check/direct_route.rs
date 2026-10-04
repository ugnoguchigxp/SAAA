//! Conversation transport selection and the direct Chat Completions attempt.
//!
//! A conversation job fixes its route once, before the tool loop. Until the user
//! saves a service registry the legacy LARM path runs unchanged; a saved registry
//! is authoritative and a failed resolution is a configuration error, never a
//! silent switch to another service.
use super::*;
use crate::providers::service_registry::{
    resolve_route, AdapterKind, Purpose, ResolveError, ResolvedRoute,
};

const DIRECT_MAX_OUTPUT_TOKENS: u32 = 4_096;

pub(super) enum ConversationTransport {
    Larm,
    Direct(ResolvedRoute),
}

pub(super) fn select_transport(state: &AppState) -> Result<ConversationTransport, String> {
    state.sqlite_readers.read(|connection| {
        if !crate::persistence::service_registry_store::has_saved_registry(connection)? {
            return Ok(ConversationTransport::Larm);
        }
        let loaded = crate::persistence::service_registry_store::load_registry(connection)?;
        transport_for(&loaded.snapshot)
    })
}

pub(super) fn transport_for(
    snapshot: &crate::providers::service_registry::RegistrySnapshot,
) -> Result<ConversationTransport, String> {
    match resolve_route(snapshot, Purpose::ConversationRespond) {
        Ok(route) => match route.adapter_kind {
            AdapterKind::ChatCompletions => Ok(ConversationTransport::Direct(route)),
            AdapterKind::Larm => Ok(ConversationTransport::Larm),
            _ => Err("選択した会話サービスの接続方式にはまだ対応していません。".into()),
        },
        // Stored and executed paths disagreed at migration time. Keep the
        // existing execution until the user applies the binding.
        Err(ResolveError::NeedsReview(_)) => Ok(ConversationTransport::Larm),
        Err(error) => Err(format!("会話サービスの設定を確認してください: {error:?}")),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn complete_direct_with_events(
    route: &ResolvedRoute,
    recent: &[context_compiler::ContextEntry],
    text: &str,
    audit: &ConversationAudit,
    context_step: &context_compiler::ContextStep<'_>,
    metrics: &context_metrics::RequestMetrics,
    on_delta: Option<&dyn crate::runtime::event_hub::RuntimeEventSender>,
    cancellation: Option<Arc<RunCancellation>>,
) -> Result<String, String> {
    let started = Instant::now();
    audit.event(
        "provider",
        "conversation-route-resolved",
        "decision",
        None,
        json!({
            "purpose": route.purpose.id(), "connectionId": route.connection_id,
            "resourceId": route.resource_id, "model": route.model,
            "fingerprint": route.fingerprint, "adapter": "chat-completions",
        }),
    );
    let authorization = match &route.credential_ref {
        Some(reference) => {
            let secret =
                crate::credentials::load_named_secret(&reference.service, &reference.account)?
                    .ok_or("会話サービスのAPIキーが登録されていません。")?;
            Some(zeroize::Zeroizing::new(format!("Bearer {}", &*secret)))
        }
        None => None,
    };
    let mut input_budget =
        crate::runtime::context::broker::ProviderInputBudget::openai_compatible();
    if context_step.mode == context_compiler::PrefixMode::Stable {
        input_budget = input_budget.with_tool_schema_reserve_bytes(0);
    }
    let metrics = metrics.for_provider(&route.endpoint);
    let compiled = context_step
        .compile(recent, text, input_budget.usable_context_bytes())
        .inspect_err(|_| metrics.invalidated())?;
    metrics.compiled(compiled.omitted);
    let timeout_ms = route.attempt_timeout_ms.unwrap_or(route.timeout_ms);
    let options = saaa_larm_session::http_api::LlmOptions {
        tools: false,
        ..Default::default()
    };
    let result = complete_http_with_instruction(
        &route.endpoint,
        authorization.as_ref().map(|value| value.as_str()),
        &route.model,
        text,
        timeout_ms,
        DIRECT_MAX_OUTPUT_TOKENS,
        Some(&options),
        false,
        Some(&compiled.instruction),
        &compiled.recent,
        Some((audit, "llm")),
        on_delta,
        cancellation,
        Some(&metrics),
    )
    .await;
    audit.event(
        "provider",
        "conversation-role-result",
        if result.is_ok() { "terminal" } else { "error" },
        Some(if result.is_ok() { "success" } else { "failure" }),
        json!({
            "resourceId": route.resource_id,
            "elapsedMs": started.elapsed().as_millis() as u64,
            "error": result.as_ref().err(),
        }),
    );
    let content = result?;
    audit.text("provider", "conversation-ornith-output", &content);
    if content.contains("<think>") || content.contains("</think>") {
        return Err("内部思考が回答本文に混入しました。".into());
    }
    Ok(content)
}
