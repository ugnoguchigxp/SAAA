use serde_json::json;
use std::{sync::Arc, time::Duration};

use super::openai_compatible::provider_api_key;
use crate::ipc_contract::ConversationMessage;
use crate::runtime::event_hub::RuntimeEventSender;
use crate::{OpenAiCompatibleProviderSettings, RunCancellation, StartTurnInput};

mod attempt;
mod dispatch;
mod dynamic_lan;
mod larm;
mod recall_dispatch;
pub(crate) use attempt::*;
pub(crate) use dispatch::*;
pub(crate) use dynamic_lan::*;
pub(crate) use larm::*;

pub(crate) struct ModelStreamContext<'a> {
    pub(crate) reasoning_effort: &'a str,
    pub(crate) max_output_tokens: u32,
    pub(crate) input: &'a StartTurnInput,
    pub(crate) on_event: &'a dyn RuntimeEventSender,
    pub(crate) cancellation: Arc<RunCancellation>,
    pub(crate) output_persistence: Option<ProviderOutputPersistence<'a>>,
}

pub(crate) async fn stream_model_provider(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    stream_model_provider_with_api_key(provider, history, timeout_ms, None, false, context).await
}

pub(crate) async fn stream_model_provider_with_api_key(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    api_key: Option<&str>,
    require_event_stream: bool,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    match stream_model_provider_inner(
        provider,
        history,
        timeout_ms,
        api_key,
        require_event_stream,
        context,
    )
    .await
    {
        Ok(content) => ProviderAttemptOutcome::Completed {
            content,
            cleanup: CleanupOutcome::NotApplicable,
        },
        Err(ProviderAttemptError::Cancelled { output_started }) => {
            ProviderAttemptOutcome::Cancelled {
                output_started,
                cleanup: CleanupOutcome::NotApplicable,
            }
        }
        Err(ProviderAttemptError::Failed {
            kind,
            output_started,
        }) => ProviderAttemptOutcome::Failed {
            kind,
            public_message: kind.public_message(),
            output_started,
            cleanup: CleanupOutcome::NotApplicable,
        },
    }
}

pub(crate) async fn stream_model_provider_inner(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    api_key: Option<&str>,
    _require_event_stream: bool,
    context: ModelStreamContext<'_>,
) -> Result<String, ProviderAttemptError> {
    if context.cancellation.is_cancelled() {
        return Err(ProviderAttemptError::Cancelled {
            output_started: false,
        });
    }
    let messages = history
        .iter()
        .filter_map(|message| {
            let role = match message.role.as_str() {
                "system" => "system",
                "assistant" => "assistant",
                "user" | "transcript" => "user",
                _ => return None,
            };
            Some(json!({ "role": role, "content": message.content }))
        })
        .collect::<Vec<_>>();
    let tools = available_agent_tools(context.output_persistence, context.input, 0, 0);
    let stream_url = provider_stream_url(&provider.endpoint)
        .map_err(|kind| ProviderAttemptError::failed(kind, false))?;
    let configured_api_key = if api_key.is_none() {
        provider_api_key(provider)
            .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Authentication, false))?
    } else {
        None
    };
    let authorization = api_key
        .or(configured_api_key.as_deref().map(String::as_str))
        .map(|credential| format!("Bearer {credential}"));
    if provider.authentication == "api-key" && authorization.is_none() {
        return Err(ProviderAttemptError::failed(
            ProviderFailureKind::Authentication,
            false,
        ));
    }
    let result = crate::providers::llm_websocket::client::run(
        crate::providers::llm_websocket::client::WebSocketRunContext {
            stream_url: stream_url.as_str(),
            authorization: authorization.as_deref(),
            allocation_id: None,
            model: &provider.model,
            messages: &messages,
            tools: &tools,
            reasoning_effort: context.reasoning_effort,
            max_output_tokens: context.max_output_tokens,
            tool_timeout: Duration::from_secs(60),
            timeout: Duration::from_millis(timeout_ms),
            input: context.input,
            on_event: context.on_event,
            cancellation: context.cancellation,
            output_persistence: context.output_persistence,
        },
    )
    .await
    .map_err(map_websocket_error)?;
    match result {
        crate::providers::llm_websocket::client::WebSocketRunResult::Completed(content) => {
            Ok(content)
        }
        crate::providers::llm_websocket::client::WebSocketRunResult::Length(_) => Err(
            ProviderAttemptError::failed(ProviderFailureKind::PartialOutput, true),
        ),
        crate::providers::llm_websocket::client::WebSocketRunResult::Failed {
            code,
            output_started,
        } => Err(ProviderAttemptError::failed(
            websocket_failure_kind(&code),
            output_started,
        )),
        crate::providers::llm_websocket::client::WebSocketRunResult::Cancelled {
            output_started,
        } => Err(ProviderAttemptError::Cancelled { output_started }),
    }
}

pub(crate) fn provider_stream_url(endpoint: &str) -> Result<url::Url, ProviderFailureKind> {
    let mut url = url::Url::parse(endpoint).map_err(|_| ProviderFailureKind::Contract)?;
    if matches!(url.scheme(), "ws" | "wss") {
        return Ok(url);
    }
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        _ => return Err(ProviderFailureKind::Contract),
    };
    url.set_scheme(scheme)
        .map_err(|_| ProviderFailureKind::Contract)?;
    let mut base_path = url.path().trim_end_matches('/');
    for suffix in ["/chat/completions", "/models", "/llm/stream"] {
        if let Some(stripped) = base_path.strip_suffix(suffix) {
            base_path = stripped;
            break;
        }
    }
    let stream_path = if base_path.is_empty() {
        "/v1/llm/stream".to_string()
    } else {
        format!("{base_path}/llm/stream")
    };
    url.set_path(&stream_path);
    Ok(url)
}

fn map_websocket_error(
    error: crate::providers::llm_websocket::client::WebSocketRunError,
) -> ProviderAttemptError {
    use crate::providers::llm_websocket::client::WebSocketRunErrorKind as WebSocket;
    let kind = match error.kind {
        WebSocket::Authentication => ProviderFailureKind::Authentication,
        WebSocket::Contract => ProviderFailureKind::Contract,
        WebSocket::Protocol => ProviderFailureKind::Protocol,
        WebSocket::Network => ProviderFailureKind::Network,
        WebSocket::Timeout => ProviderFailureKind::Timeout,
        WebSocket::ClientDisconnected => ProviderFailureKind::ClientDisconnected,
        WebSocket::Internal => ProviderFailureKind::Internal,
    };
    ProviderAttemptError::failed(kind, error.output_started)
}

fn websocket_failure_kind(code: &str) -> ProviderFailureKind {
    match code {
        "authentication" | "not-authorized" => ProviderFailureKind::Authentication,
        "invalid-request" | "protocol" => ProviderFailureKind::Protocol,
        "request-too-large"
        | "response-too-large"
        | "tool-arguments-too-large"
        | "tool-result-too-large" => ProviderFailureKind::RequestTooLarge,
        "allocation-lost" => ProviderFailureKind::AllocationLost,
        "capacity" => ProviderFailureKind::Capacity,
        "model-unavailable" => ProviderFailureKind::Unavailable,
        "timeout" | "tool-timeout" => ProviderFailureKind::Timeout,
        "upstream" | "backpressure" => ProviderFailureKind::Upstream,
        _ => ProviderFailureKind::Internal,
    }
}

#[cfg(test)]
mod websocket_url_tests {
    use super::*;

    #[test]
    fn http_provider_endpoints_project_to_one_canonical_websocket_path() {
        for endpoint in [
            "http://127.0.0.1:9000/v1",
            "http://127.0.0.1:9000/v1/chat/completions",
            "http://127.0.0.1:9000/v1/models",
            "http://127.0.0.1:9000/v1/llm/stream",
        ] {
            assert_eq!(
                provider_stream_url(endpoint).unwrap().as_str(),
                "ws://127.0.0.1:9000/v1/llm/stream"
            );
        }
    }
}
