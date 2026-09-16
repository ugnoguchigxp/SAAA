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
) -> ProviderAttemptOutcome {
    if shared_voice_session {
        stream_larm_voice_provider(
            settings,
            provider.request_options.clone(),
            conversation_id,
            history,
            timeout_ms,
            context,
        )
        .await
    } else {
        super::stream_dynamic_lan_provider(
            provider,
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
) -> ProviderAttemptOutcome {
    let cancellation = context.cancellation.clone();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let ready = tokio::select! { biased;
        _ = cancellation.cancelled() => return cancelled(),
        result = tokio::time::timeout_at(deadline, crate::larm_voice::current_at(conversation_id, settings)) => match result {
            Ok(Ok(ready)) => ready,
            Ok(Err(_)) => return failed(ProviderFailureKind::Unavailable),
            Err(_) => return failed(ProviderFailureKind::Timeout),
        }
    };
    let lease = tokio::select! { biased;
        _ = cancellation.cancelled() => return cancelled(),
        result = tokio::time::timeout_at(deadline, ready.session.acquire("llm")) => match result {
            Ok(Ok(lease)) => lease,
            Ok(Err(_)) => return failed(ProviderFailureKind::Unavailable),
            Err(_) => return failed(ProviderFailureKind::Timeout),
        }
    };
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
    let resolved = OpenAiCompatibleProviderSettings {
        request_options,
        id: crate::DYNAMIC_LAN_PROVIDER_ID.to_string(),
        enabled: true,
        label: "LARM conversation reasoning".to_string(),
        location: "local".to_string(),
        endpoint: provider.base_url.to_string(),
        model: provider.model.clone(),
        authentication: "api-key".to_string(),
    };
    stream_model_provider_with_api_key(
        &resolved,
        history,
        request_timeout,
        Some(provider.token()),
        Some(lease.allocation_id()),
        context,
    )
    .await
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
