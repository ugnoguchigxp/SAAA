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
    Larm(Option<ResolvedRoute>),
    Direct(ResolvedRoute),
}

pub(super) fn prepare_transport(
    state: &AppState,
) -> Result<
    (
        crate::ModelProvidersSettings,
        u64,
        ConversationTransport,
        Vec<ResolvedRoute>,
    ),
    String,
> {
    state.sqlite_readers.read(|db| {
        let providers = persistence::load_model_providers(db)?;
        let timeout = persistence::load_routing_settings(db)?
            .conversation_respond
            .timeout_ms;
        let loaded = persistence::service_registry_store::load_registry(db)?;
        let transport = if loaded.persisted {
            transport_for(&loaded.snapshot)?
        } else {
            legacy_harness(&loaded.snapshot)?
        };
        let fallbacks = match &transport {
            ConversationTransport::Direct(route) => route
                .fallback_resource_ids
                .iter()
                .map(|id| {
                    crate::providers::service_registry::resolve_resource(
                        &loaded.snapshot,
                        route.purpose,
                        id,
                    )
                    .map_err(|e| format!("会話の代替先設定を確認してください: {e:?}"))
                })
                .collect::<Result<Vec<_>, _>>()?,
            _ => Vec::new(),
        };
        Ok((providers, timeout, transport, fallbacks))
    })
}

fn legacy_harness(
    snapshot: &crate::providers::service_registry::RegistrySnapshot,
) -> Result<ConversationTransport, String> {
    let mut snapshot = snapshot.clone();
    let binding = snapshot
        .bindings
        .iter_mut()
        .find(|b| b.purpose == Purpose::ConversationRespond)
        .ok_or("会話設定がありません")?;
    binding.enabled = true;
    binding.review = crate::providers::service_registry::BindingReview::Ready;
    binding.primary_resource_id = Some("res:harness-llm".into());
    binding.fallback_resource_ids.clear();
    resolve_route(&snapshot, Purpose::ConversationRespond)
        .map(|route| ConversationTransport::Larm(Some(route)))
        .map_err(|e| format!("LARMの設定を確認してください: {e:?}"))
}

pub(super) fn transport_for(
    snapshot: &crate::providers::service_registry::RegistrySnapshot,
) -> Result<ConversationTransport, String> {
    match resolve_route(snapshot, Purpose::ConversationRespond) {
        Ok(route) => match route.adapter_kind {
            AdapterKind::ChatCompletions | AdapterKind::AnthropicMessages => {
                Ok(ConversationTransport::Direct(route))
            }
            AdapterKind::Larm => Ok(ConversationTransport::Larm(Some(route))),
            _ => Err("選択した会話サービスの接続方式にはまだ対応していません。".into()),
        },
        // Stored and executed paths disagreed at migration time. Keep the
        // existing execution until the user applies the binding.
        Err(ResolveError::NeedsReview(_)) => legacy_harness(snapshot),
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
    fallback_safe: &AtomicBool,
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
    let mut options = route
        .request_options
        .clone()
        .map(serde_json::from_value::<saaa_larm_session::http_api::LlmOptions>)
        .transpose()
        .map_err(|e| format!("会話モデルの互換設定が不正です: {e}"))?
        .unwrap_or_default();
    options.tools = false;
    audit
        .writer
        .read_serialized(|db| validate_route(db, route))?;
    if route.adapter_kind == AdapterKind::AnthropicMessages {
        let result = crate::providers::anthropic_messages::run(
            &route.endpoint,
            authorization
                .as_ref()
                .and_then(|v| v.strip_prefix("Bearer ")),
            &route.model,
            &compiled.instruction,
            &compiled.recent,
            text,
            timeout_ms,
            DIRECT_MAX_OUTPUT_TOKENS,
            cancellation.unwrap_or_default(),
        )
        .await;
        return match result {
            Ok(response) => {
                metrics.response_model(response.model.as_deref());
                audit.event("provider","conversation-native-result","terminal",Some("success"),json!({"resourceId":route.resource_id,"model":response.model,"requestId":response.request_id,"usage":response.usage}));
                if let Some(on_delta) = on_delta {
                    on_delta
                        .send(crate::ipc_contract::RuntimeEvent::Delta {
                            run_id: audit.correlation_id.clone(),
                            text: response.content.clone(),
                        })
                        .map_err(|_| "回答を受け渡せません")?;
                }
                Ok(response.content)
            }
            Err(crate::providers::stream::ProviderAttemptError::Failed {
                kind,
                output_started,
                ..
            }) => {
                fallback_safe.store(
                    !output_started
                        && matches!(
                            kind,
                            crate::providers::stream::ProviderFailureKind::Connect
                                | crate::providers::stream::ProviderFailureKind::Capacity
                                | crate::providers::stream::ProviderFailureKind::Unavailable
                        ),
                    Ordering::Release,
                );
                Err(kind.public_message().as_str().to_string())
            }
            Err(_) => Err("Providerの処理が中断されました。".into()),
        };
    }
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
        Some(fallback_safe),
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

/// Ordinary selection edits affect the next job. Revocations affect this send.
pub(super) fn validate_transport(
    state: &AppState,
    transport: &ConversationTransport,
) -> Result<(), String> {
    let route = match transport {
        ConversationTransport::Direct(route) | ConversationTransport::Larm(Some(route)) => route,
        ConversationTransport::Larm(None) => return Ok(()),
    };
    state.sqlite_readers.read(|db| validate_route(db, route))
}

pub(super) use crate::providers::service_registry::validate_active as validate_route;

pub(super) fn route_of(transport: &ConversationTransport) -> Option<&ResolvedRoute> {
    match transport {
        ConversationTransport::Direct(r) | ConversationTransport::Larm(Some(r)) => Some(r),
        _ => None,
    }
}

/// Dropped attempts (deadline, shutdown, cancellation) retain a terminal failure.
pub(super) struct RouteAttempt {
    audit: ConversationAudit,
    attributes: String,
    finished: bool,
}

impl RouteAttempt {
    pub(super) fn begin(audit: &ConversationAudit, route: &ResolvedRoute) -> Result<Self, String> {
        let id = format!("attempt_{}", uuid::Uuid::new_v4().simple());
        let attributes =
            crate::providers::service_registry::operations::attributes(route, Some(&id))
                .to_string();
        audit.write_event(
            "provider",
            "purpose-route-attempt",
            "start",
            None,
            &attributes,
        )?;
        Ok(Self {
            audit: audit.clone(),
            attributes,
            finished: false,
        })
    }

    pub(super) fn finish(&mut self, success: bool) -> Result<(), String> {
        self.audit.write_event(
            "provider",
            "purpose-route-attempt",
            "terminal",
            Some(if success { "success" } else { "failure" }),
            &self.attributes,
        )?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for RouteAttempt {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.finish(false);
        }
    }
}
