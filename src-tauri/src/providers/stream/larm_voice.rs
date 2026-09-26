//! Conversation inference through the voice session's shared LARM connection.
//! The 27B provider remains the model/tool planner; SAAA keeps tool execution.
use super::{stream_model_provider_with_api_key, ModelStreamContext};
use crate::ipc_contract::ConversationMessage;
use crate::{
    CleanupOutcome, DynamicLanProviderSettings, HarnessSettings, OpenAiCompatibleProviderSettings,
    ProviderAttemptOutcome, ProviderFailureKind,
};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_voice_aware_dynamic_lan_provider(
    provider: &DynamicLanProviderSettings,
    settings: &HarnessSettings,
    shared_voice_session: bool,
    conversation_id: &str,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    larm_provider: &'static str,
) -> ProviderAttemptOutcome {
    if larm_provider == "backchannel" && !shared_voice_session {
        return failed(ProviderFailureKind::Contract);
    }
    if shared_voice_session {
        stream_larm_voice_provider(
            settings,
            provider.request_options.clone(),
            conversation_id,
            history,
            timeout_ms,
            context,
            larm_provider,
        )
        .await
    } else if let Some(persistence) = context.output_persistence {
        if crate::larm_voice::ensure_conversation_owner(
            conversation_id,
            settings,
            persistence.state.sqlite_writer.clone(),
        )
        .await
        .is_err()
        {
            return failed(ProviderFailureKind::Unavailable);
        }
        stream_larm_voice_provider(
            settings,
            provider.request_options.clone(),
            conversation_id,
            history,
            timeout_ms,
            context,
            larm_provider,
        )
        .await
    } else {
        super::stream_dynamic_lan_provider(
            provider,
            settings.larm_profile.as_deref(),
            history,
            timeout_ms.min(crate::providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS),
            context.cancellation.clone(),
            context,
        )
        .await
    }
}

async fn stream_larm_voice_provider(
    settings: &HarnessSettings,
    request_options: Option<saaa_larm_session::http_api::LlmOptions>,
    conversation_id: &str,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    larm_provider: &'static str,
) -> ProviderAttemptOutcome {
    let started = tokio::time::Instant::now();
    let deadline = started + std::time::Duration::from_millis(timeout_ms);
    let role_handoff = match is_role_handoff(&context) {
        Ok(value) => value,
        Err(_) => return failed(ProviderFailureKind::Contract),
    };
    let handoff_connection_deadline =
        role_handoff.then(|| started + std::time::Duration::from_secs(25));
    let mut used = None;
    let first = stream_larm_voice_provider_once(
        settings,
        request_options.clone(),
        conversation_id,
        history,
        timeout_ms,
        context.clone(),
        larm_provider,
        &mut used,
        handoff_connection_deadline,
    )
    .await;
    if matches!(
        first,
        ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::AllocationLost,
            output_started: false,
            ..
        }
    ) {
        // A rejected first connection is already closed by connect_inner. A leased connection
        // needs explicit invalidation before the same one-shot retry.
        let may_retry = match used {
            Some(session) => crate::larm_voice::invalidate_connection(conversation_id, &session)
                .await
                .is_ok(),
            None => true,
        };
        if may_retry {
            let remaining_ms = deadline
                .saturating_duration_since(tokio::time::Instant::now())
                .as_millis() as u64;
            if remaining_ms == 0 {
                return failed(ProviderFailureKind::Timeout);
            }
            let mut retry_used = None;
            return stream_larm_voice_provider_once(
                settings,
                request_options,
                conversation_id,
                history,
                remaining_ms,
                context,
                larm_provider,
                &mut retry_used,
                handoff_connection_deadline,
            )
            .await;
        }
    }
    first
}

#[allow(clippy::too_many_arguments)]
async fn stream_larm_voice_provider_once(
    settings: &HarnessSettings,
    request_options: Option<saaa_larm_session::http_api::LlmOptions>,
    conversation_id: &str,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    larm_provider: &'static str,
    used: &mut Option<std::sync::Arc<saaa_larm_session::Session>>,
    handoff_connection_deadline: Option<tokio::time::Instant>,
) -> ProviderAttemptOutcome {
    let cancellation = context.cancellation.clone();
    let attempt_started = tokio::time::Instant::now();
    let deadline = attempt_started + std::time::Duration::from_millis(timeout_ms);
    // A cold connection must leave enough of the step budget for an actual answer.
    let connection_deadline =
        connection_wait_deadline(attempt_started, timeout_ms, handoff_connection_deadline);
    record_reasoner_stage(&context, "ornith-connection-started", "start", None, None);
    if cancellation.is_cancelled() {
        record_reasoner_stage(
            &context,
            "ornith-connection-finished",
            "terminal",
            Some("cancelled"),
            Some("user-cancelled"),
        );
        return cancelled();
    }
    if connection_deadline <= tokio::time::Instant::now() {
        record_reasoner_stage(
            &context,
            "ornith-connection-finished",
            "terminal",
            Some("failure"),
            Some("preparation-deferred"),
        );
        return failed(ProviderFailureKind::PreparationDeferred);
    }
    let ready = tokio::select! { biased;
        _ = cancellation.cancelled() => {
            record_reasoner_stage(&context, "ornith-connection-finished", "terminal", Some("cancelled"), Some("user-cancelled"));
            return cancelled();
        },
        result = tokio::time::timeout_at(connection_deadline, crate::larm_voice::current_at(conversation_id, settings)) => match result {
            Ok(Ok(ready)) => ready,
            Ok(Err(error)) if matches!(error.as_str(), "larm_connection_idle_released" | "larm_expired") => {
                record_reasoner_stage(&context, "ornith-connection-finished", "terminal", Some("failure"), Some("allocation-lost"));
                return failed(ProviderFailureKind::AllocationLost);
            }
            Ok(Err(_)) => {
                record_reasoner_stage(&context, "ornith-connection-finished", "terminal", Some("failure"), Some("unavailable"));
                return failed(ProviderFailureKind::Unavailable);
            }
            Err(_) => {
                record_reasoner_stage(&context, "ornith-connection-finished", "terminal", Some("failure"), Some("preparation-deferred"));
                return failed(ProviderFailureKind::PreparationDeferred);
            }
        }
    };
    record_reasoner_stage(
        &context,
        "ornith-connection-finished",
        "terminal",
        Some("success"),
        None,
    );
    record_reasoner_ids(
        &context,
        "ornith-connection-claimed",
        &[("agentConnectionId", ready.session.connection_id())],
    );
    *used = Some(ready.session.clone());
    record_reasoner_stage(&context, "ornith-lease-started", "start", None, None);
    let lease = tokio::select! { biased;
        _ = cancellation.cancelled() => {
            record_reasoner_stage(&context, "ornith-lease-finished", "terminal", Some("cancelled"), Some("user-cancelled"));
            return cancelled();
        },
        result = tokio::time::timeout_at(deadline, ready.session.acquire(larm_provider)) => match result {
            Ok(Ok(lease)) => lease,
            Ok(Err("larm_session_closed" | "larm_connection_idle_released" | "larm_expired")) => {
                record_reasoner_stage(&context, "ornith-lease-finished", "terminal", Some("failure"), Some("allocation-lost"));
                return failed(ProviderFailureKind::AllocationLost);
            }
            Ok(Err(_)) => {
                record_reasoner_stage(&context, "ornith-lease-finished", "terminal", Some("failure"), Some("unavailable"));
                return failed(ProviderFailureKind::Unavailable);
            }
            Err(_) => {
                record_reasoner_stage(&context, "ornith-lease-finished", "terminal", Some("failure"), Some("timeout"));
                return failed(ProviderFailureKind::Timeout);
            }
        }
    };
    record_reasoner_stage(
        &context,
        "ornith-lease-finished",
        "terminal",
        Some("success"),
        None,
    );
    record_reasoner_ids(
        &context,
        "ornith-lease-acquired",
        &[
            ("agentConnectionId", ready.session.connection_id()),
            ("allocationId", lease.allocation_id()),
        ],
    );
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return failed(ProviderFailureKind::Timeout);
    }
    let request_timeout = match lease.request_budget(remaining) {
        Ok(timeout) if !timeout.is_zero() => {
            timeout.as_millis().try_into().unwrap_or(u64::MAX).max(1)
        }
        _ => return failed(ProviderFailureKind::AllocationLost),
    };
    let provider = lease.provider();
    let Some(context_window) = provider.context_window else {
        return failed(ProviderFailureKind::Contract);
    };
    if context.max_output_tokens == 0
        || u64::from(context.max_output_tokens) > context_window.output_reserve_tokens
    {
        return failed(ProviderFailureKind::Contract);
    }
    let mut options = request_options.unwrap_or_default();
    options.thinking = saaa_larm_session::http_api::Thinking::Disabled;
    let resolved = OpenAiCompatibleProviderSettings {
        request_options: Some(options),
        id: crate::DYNAMIC_LAN_PROVIDER_ID.to_string(),
        enabled: true,
        label: "LARM conversation reasoning".to_string(),
        location: "local".to_string(),
        endpoint: provider.base_url.to_string(),
        model: provider.model.clone(),
        authentication: "api-key".to_string(),
    };
    record_reasoner_stage(
        &context,
        "ornith-generation-request-started",
        "start",
        None,
        None,
    );
    let outcome = stream_model_provider_with_api_key(
        &resolved,
        history,
        request_timeout,
        Some(provider.token()),
        Some(lease.allocation_id()),
        context.clone(),
    )
    .await;
    let (result, failure) = match &outcome {
        ProviderAttemptOutcome::Completed { .. } => ("success", None),
        ProviderAttemptOutcome::Cancelled { .. } => ("cancelled", Some("user-cancelled")),
        ProviderAttemptOutcome::Failed { kind, .. } => ("failure", Some(kind.as_str())),
    };
    record_reasoner_stage(
        &context,
        "ornith-generation-request-finished",
        "terminal",
        Some(result),
        failure,
    );
    outcome
}

fn record_reasoner_stage(
    context: &ModelStreamContext<'_>,
    name: &str,
    phase: &str,
    outcome: Option<&str>,
    failure_code: Option<&str>,
) {
    let Some(persistence) = context.output_persistence else {
        return;
    };
    let input = context.input;
    let _ = crate::persistence::audit::record_frontend_event(
        persistence.state,
        &crate::persistence::audit::FrontendAuditEventInput {
            component: "provider".into(),
            event_name: name.into(),
            phase: phase.into(),
            outcome: outcome.map(str::to_string),
            correlation_id: Some(input.run_id.clone()),
            causation_id: None,
            conversation_id: Some(input.conversation_id.clone()),
            runtime_run_id: Some(input.run_id.clone()),
            session_id: Some(persistence.session_id.to_string()),
            subject_id: Some(input.run_id.clone()),
            failure_code: failure_code.map(str::to_string),
            attributes: std::collections::BTreeMap::new(),
        },
    );
}

fn record_reasoner_ids(context: &ModelStreamContext<'_>, name: &str, ids: &[(&str, &str)]) {
    let Some(persistence) = context.output_persistence else {
        return;
    };
    let input = context.input;
    let attributes = ids
        .iter()
        .map(|(key, value)| {
            (
                (*key).to_string(),
                crate::persistence::audit::AuditAttributeValue::Tag((*value).to_string()),
            )
        })
        .collect();
    let _ = crate::persistence::audit::record_frontend_event(
        persistence.state,
        &crate::persistence::audit::FrontendAuditEventInput {
            component: "provider".into(),
            event_name: name.into(),
            phase: "progress".into(),
            outcome: None,
            correlation_id: Some(input.run_id.clone()),
            causation_id: None,
            conversation_id: Some(input.conversation_id.clone()),
            runtime_run_id: Some(input.run_id.clone()),
            session_id: Some(persistence.session_id.to_string()),
            subject_id: Some(input.run_id.clone()),
            failure_code: None,
            attributes,
        },
    );
}

fn connection_wait_deadline(
    attempt_started: tokio::time::Instant,
    timeout_ms: u64,
    handoff_deadline: Option<tokio::time::Instant>,
) -> tokio::time::Instant {
    match handoff_deadline {
        Some(handoff) => handoff.min(
            attempt_started + std::time::Duration::from_millis(timeout_ms.saturating_sub(30_000)),
        ),
        None => attempt_started + std::time::Duration::from_millis(timeout_ms),
    }
}

fn is_role_handoff(context: &ModelStreamContext<'_>) -> Result<bool, String> {
    if context.input.input_origin != "voice" {
        return Ok(false);
    }
    let Some(persistence) = context.output_persistence else {
        return Ok(false);
    };
    persistence.state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM rr_steps WHERE root_id=?1 AND purpose='respond')",
                [&context.input.run_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| error.to_string())
    })
}

#[cfg(test)]
mod budget_tests {
    use super::connection_wait_deadline;

    #[test]
    fn connection_wait_preserves_generation_budget_and_is_bounded() {
        let start = tokio::time::Instant::now();
        let handoff = start + std::time::Duration::from_secs(25);
        assert_eq!(
            connection_wait_deadline(start, 40_000, Some(handoff)),
            start + std::time::Duration::from_secs(10)
        );
        assert_eq!(
            connection_wait_deadline(start, 120_000, Some(handoff)),
            handoff
        );
        assert_eq!(
            connection_wait_deadline(start, 10_000, Some(handoff)),
            start
        );
        assert_eq!(
            connection_wait_deadline(start, 10_000, None),
            start + std::time::Duration::from_secs(10)
        );
        let after_retry = start + std::time::Duration::from_secs(20);
        assert_eq!(
            connection_wait_deadline(after_retry, 100_000, Some(handoff)),
            handoff
        );
    }
}

fn cancelled() -> ProviderAttemptOutcome {
    ProviderAttemptOutcome::Cancelled {
        output_started: false,
        cleanup: CleanupOutcome::NotApplicable,
    }
}

fn failed(kind: ProviderFailureKind) -> ProviderAttemptOutcome {
    ProviderAttemptOutcome::Failed {
        kind,
        public_message: kind.public_message(),
        output_started: false,
        cleanup: CleanupOutcome::NotApplicable,
    }
}
