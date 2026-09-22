use std::sync::Arc;
use zeroize::Zeroizing;

use super::openai_compatible::provider_api_key;
use crate::ipc_contract::ConversationMessage;
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{OpenAiCompatibleProviderSettings, RunCancellation, StartTurnInput};

mod agent_dispatch;
mod attempt;
mod dispatch;
mod dynamic_lan;
mod larm_voice;
mod recall_dispatch;
pub(crate) use attempt::*;
pub(crate) use dispatch::*;
pub(crate) use dynamic_lan::*;
pub(crate) use larm_voice::*;
#[cfg(test)]
pub(crate) use recall_dispatch::execute_recall_tool;

pub(crate) struct ModelStreamContext<'a> {
    pub(crate) reasoning_effort: &'a str,
    pub(crate) max_output_tokens: u32,
    pub(crate) input: &'a StartTurnInput,
    pub(crate) on_event: &'a dyn RuntimeEventSender,
    pub(crate) cancellation: Arc<RunCancellation>,
    pub(crate) context_health: &'a str,
    pub(crate) context_sources: &'a [crate::runtime::context::source::Candidate],
    pub(crate) context_omissions: &'a [crate::runtime::context::source::Candidate],
    pub(crate) output_persistence: Option<ProviderOutputPersistence<'a>>,
}

pub(crate) async fn stream_model_provider(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    stream_model_provider_with_api_key(provider, history, timeout_ms, None, None, context).await
}

pub(crate) async fn stream_model_provider_with_api_key(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    api_key: Option<&str>,
    _allocation_id: Option<&str>,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    provider_attempt_outcome(
        stream_model_provider_inner(
            provider,
            history,
            timeout_ms,
            api_key,
            _allocation_id,
            context,
        )
        .await,
        CleanupOutcome::NotApplicable,
    )
}

fn provider_attempt_outcome(
    result: Result<String, ProviderAttemptError>,
    cleanup: CleanupOutcome,
) -> ProviderAttemptOutcome {
    match result {
        Ok(content) => ProviderAttemptOutcome::Completed { content, cleanup },
        Err(ProviderAttemptError::Cancelled { output_started }) => {
            ProviderAttemptOutcome::Cancelled {
                output_started,
                cleanup,
            }
        }
        Err(ProviderAttemptError::Failed {
            kind,
            output_started,
        }) => ProviderAttemptOutcome::Failed {
            kind,
            public_message: kind.public_message(),
            output_started,
            cleanup,
        },
    }
}

pub(crate) async fn stream_model_provider_inner(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    api_key: Option<&str>,
    allocation_id: Option<&str>,
    context: ModelStreamContext<'_>,
) -> Result<String, ProviderAttemptError> {
    if context.cancellation.is_cancelled() {
        return Err(ProviderAttemptError::Cancelled {
            output_started: false,
        });
    }
    let configured_api_key = if api_key.is_none() {
        provider_api_key(provider)
            .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Authentication, false))?
    } else {
        None
    };
    let authorization = api_key
        .or(configured_api_key.as_deref().map(String::as_str))
        .map(|credential| Zeroizing::new(format!("Bearer {credential}")));
    if provider.authentication == "api-key" && authorization.is_none() {
        return Err(ProviderAttemptError::failed(
            ProviderFailureKind::Authentication,
            false,
        ));
    }
    let options = provider.request_options.clone().unwrap_or_else(|| {
        let mut options = saaa_larm_session::http_api::LlmOptions::standard();
        options.streaming = allocation_id.is_none();
        options
    });
    if let Some(persistence) = context.output_persistence {
        persistence.bind_transport(allocation_id);
    }
    crate::providers::chat_completions::run_with_options(
        &provider.endpoint,
        authorization.as_deref().map(String::as_str),
        &provider.model,
        history,
        timeout_ms,
        context,
        crate::providers::chat_completions::RequestMode::Stream,
        &options,
    )
    .await
}
