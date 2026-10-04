//! Initial fallback and attempt execution for one pinned conversation step.
use super::*;
pub(super) struct Input<'a, 'b, R: Runtime> {
    pub state: &'a AppState,
    pub session: Option<&'a Arc<saaa_larm_session::Session>>,
    pub transport: &'a mut direct_route::ConversationTransport,
    pub fallbacks: &'a mut std::vec::IntoIter<crate::providers::service_registry::ResolvedRoute>,
    pub deadline: tokio::time::Instant,
    pub step: usize,
    pub timeout: u64,
    pub recent: &'a [context_compiler::ContextEntry],
    pub text: &'a str,
    pub audit: &'a ConversationAudit,
    pub context_step: &'a context_compiler::ContextStep<'b>,
    pub metrics: &'a context_metrics::RequestMetrics,
    pub deltas: &'a queue_answer_stream::AnswerDeltaSender<R>,
    pub cancellation: &'a Arc<RunCancellation>,
    pub retry_blocked: &'a Arc<AtomicBool>,
}
pub(super) async fn complete<R: Runtime>(
    input: Input<'_, '_, R>,
) -> Result<(String, String), String> {
    let Input {
        state,
        session,
        transport,
        fallbacks,
        deadline,
        step,
        timeout,
        recent,
        text,
        audit,
        context_step,
        metrics,
        deltas,
        cancellation,
        retry_blocked,
    } = input;
    loop {
        direct_route::validate_transport(state, &*transport).inspect_err(|_| {
            cancellation.cancel();
            retry_blocked.store(true, Ordering::Release);
        })?;
        let fallback_safe = AtomicBool::new(false);
        let mut attempt = direct_route::route_of(&*transport)
            .map(|route| direct_route::RouteAttempt::begin(audit, route))
            .transpose()
            .inspect_err(|_| {
                cancellation.cancel();
                retry_blocked.store(true, Ordering::Release);
            })?;
        let remaining = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .as_millis() as u64;
        if remaining == 0 {
            cancellation.cancel();
            retry_blocked.store(true, Ordering::Release);
            return Err("この依頼の全体期限を超えました".into());
        }
        let attempt_budget = direct_route::route_of(&*transport)
            .map(|r| r.attempt_timeout_ms.unwrap_or(r.timeout_ms))
            .unwrap_or(timeout)
            .min(remaining);
        let completion = match (session, &*transport) {
            (Some(session), _) => {
                complete_larm_role_with_events(
                    session,
                    "llm",
                    recent,
                    text,
                    attempt_budget,
                    audit,
                    context_step,
                    metrics,
                    Some(deltas),
                    Some(cancellation.clone()),
                )
                .await
            }
            (None, direct_route::ConversationTransport::Direct(route)) => {
                let mut attempt = route.clone();
                attempt.attempt_timeout_ms = Some(attempt_budget);
                direct_route::complete_direct_with_events(
                    &attempt,
                    recent,
                    text,
                    audit,
                    context_step,
                    metrics,
                    Some(deltas),
                    Some(cancellation.clone()),
                    &fallback_safe,
                )
                .await
                .map(|content| {
                    (
                        content,
                        metrics
                            .observed_model()
                            .unwrap_or_else(|| route.model.clone()),
                    )
                })
            }
            (None, direct_route::ConversationTransport::Larm(_)) => {
                Err("会話の接続先を準備できませんでした。".to_string())
            }
        };
        if let Some(attempt) = &mut attempt {
            attempt.finish(completion.is_ok()).inspect_err(|_| {
                cancellation.cancel();
                retry_blocked.store(true, Ordering::Release);
            })?;
        }
        if completion.is_err()
            && step == 0
            && fallback_safe.load(Ordering::Acquire)
            && !retry_blocked.load(Ordering::Acquire)
            && !cancellation.is_cancelled()
        {
            if let Some(next) = fallbacks.next() {
                audit.event("provider", "purpose-route-fallback", "decision", None,
                        json!({"purpose": next.purpose.id(), "resourceId":next.resource_id,"reason":"initial-attempt-rejected-before-output"}));
                *transport = direct_route::ConversationTransport::Direct(next);
                continue;
            }
        }
        if completion.is_err() && step == 0 {
            retry_blocked.store(true, Ordering::Release);
        }
        break completion;
    }
}
