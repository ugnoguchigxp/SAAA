//! Concrete provider dispatch with the same context and receipt binding.
use super::*;
pub(super) async fn attempt(
    provider: &ModelProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    harness: &crate::HarnessSettings,
    shared_larm_voice: bool,
    larm_provider: &'static str,
) -> ProviderAttemptOutcome {
    match provider {
        ModelProviderSettings::OpenAiCompatible(provider) => {
            stream_model_provider(provider, history, timeout_ms, context).await
        }
        ModelProviderSettings::AgentSession(provider) => {
            if context.output_persistence.is_some_and(|persistence| {
                crate::runtime::image_input::has_claimed(
                    &persistence.state.data_directory,
                    &context.input.run_id,
                )
            }) {
                return ProviderAttemptOutcome::Failed {
                    kind: ProviderFailureKind::Contract,
                    public_message:
                        crate::providers::stream::BoundedProviderMessage::from_static_diagnostic(
                            "This provider cannot receive the attached image.",
                        ),
                    output_started: false,
                    cleanup: CleanupOutcome::NotApplicable,
                };
            }
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
                larm_provider,
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
