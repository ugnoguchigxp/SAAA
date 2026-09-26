//! Small tool-less Chat Completions transport for provider diagnostics and generation.
//! The removed conversation executor is not reintroduced here.
use super::stream::{ModelStreamContext, ProviderAttemptError, ProviderFailureKind};
use crate::ipc_contract::{ConversationMessage, RuntimeEvent};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestMode {
    Stream,
    JsonProbe,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_with_options(
    endpoint: &str,
    authorization: Option<&str>,
    model: &str,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    mode: RequestMode,
    options: &saaa_larm_session::http_api::LlmOptions,
) -> Result<String, ProviderAttemptError> {
    use ProviderFailureKind as Failure;
    if context.cancellation.is_cancelled() {
        return Err(ProviderAttemptError::Cancelled {
            output_started: false,
        });
    }
    // These require the replacement conversation host. Silently omitting context, tools, or
    // persistence would produce an answer without its required safety and audit boundary.
    if context.output_persistence.is_some()
        || !context.context_sources.is_empty()
        || (mode == RequestMode::Stream && options.tools)
    {
        return Err(ProviderAttemptError::failed(Failure::Unavailable, false));
    }
    let url = super::openai_compatible::provider_operation_url(endpoint, "chat/completions")
        .map_err(|_| ProviderAttemptError::failed(Failure::Contract, false))?;
    let messages: Vec<Value> = history
        .iter()
        .map(|message| json!({"role": message.role, "content": message.content}))
        .collect();
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": false,
        "max_tokens": context.max_output_tokens,
    });
    options.apply(
        &mut body,
        model,
        context.max_output_tokens,
        context.reasoning_effort,
    );
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| ProviderAttemptError::failed(Failure::Internal, false))?;
    let mut request = client.post(url).json(&body);
    if let Some(authorization) = authorization {
        request = request.header(reqwest::header::AUTHORIZATION, authorization);
    }
    let cancellation = context.cancellation.clone();
    let response = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(ProviderAttemptError::Cancelled { output_started: false }),
        result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
            let mut response = request.send().await.map_err(|_| Failure::Network)?;
            if !response.status().is_success() {
                return Err(super::http::status_failure(response.status().as_u16()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| Failure::ResponseInterrupted)? {
                if bytes.len().saturating_add(chunk.len()) > 2 * 1024 * 1024 {
                    return Err(Failure::RequestTooLarge);
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice::<Value>(&bytes).map_err(|_| Failure::Protocol)
        }) => result.map_err(|_| ProviderAttemptError::failed(Failure::Timeout, false))?
            .map_err(|kind| ProviderAttemptError::failed(kind, false))?,
    };
    let choice = response["choices"]
        .as_array()
        .and_then(|choices| (choices.len() == 1).then(|| &choices[0]))
        .ok_or_else(|| ProviderAttemptError::failed(Failure::Protocol, false))?;
    if choice["finish_reason"] != "stop" || !choice["message"]["tool_calls"].is_null() {
        return Err(ProviderAttemptError::failed(Failure::Protocol, false));
    }
    let content = choice["message"]["content"]
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| ProviderAttemptError::failed(Failure::Protocol, false))?
        .to_string();
    if mode == RequestMode::Stream {
        context
            .on_event
            .send(RuntimeEvent::Delta {
                run_id: context.input.run_id.clone(),
                text: content.clone(),
            })
            .map_err(|_| ProviderAttemptError::failed(Failure::ClientDisconnected, true))?;
    }
    Ok(content)
}
