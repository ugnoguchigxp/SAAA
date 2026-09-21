//! Concrete provider dispatch with the same context and receipt binding.
use super::*;
pub(super) async fn attempt(
    provider: &ModelProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    harness: &crate::HarnessSettings,
    shared_larm_voice: bool,
) -> ProviderAttemptOutcome {
    match provider {
        ModelProviderSettings::OpenAiCompatible(provider) => {
            stream_model_provider(provider, history, timeout_ms, context).await
        }
        ModelProviderSettings::AgentSession(provider) => {
            crate::providers::agent_session::stream_agent_session_provider(
                provider, history, timeout_ms, context,
            )
            .await
        }
        ModelProviderSettings::DynamicLan(provider) => {
            stream_voice_aware_dynamic_lan_provider(
                provider,
                harness,
                shared_larm_voice,
                &context.input.conversation_id,
                history,
                timeout_ms,
                context,
            )
            .await
        }
        _ => ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::Contract,
            public_message: ProviderFailureKind::Contract.public_message(),
            output_started: false,
            cleanup: CleanupOutcome::NotApplicable,
        },
    }
}
