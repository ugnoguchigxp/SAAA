//! Small tool-less Chat Completions transport for provider diagnostics and generation.
//! The removed conversation executor is not reintroduced here.
use super::stream::{ModelStreamContext, ProviderAttemptError, ProviderFailureKind};
use crate::ipc_contract::ConversationMessage;
use serde_json::{json, Value};
use std::time::Duration;

mod chunks;
mod json_response;
pub(crate) mod observation;
mod response_json;
mod sse;
mod stream_response;

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
    run_with_proxy_policy(
        endpoint,
        authorization,
        model,
        history,
        timeout_ms,
        context,
        mode,
        options,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_with_proxy_policy(
    endpoint: &str,
    authorization: Option<&str>,
    model: &str,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    mode: RequestMode,
    options: &saaa_larm_session::http_api::LlmOptions,
    no_proxy: bool,
) -> Result<String, ProviderAttemptError> {
    run_observed(
        endpoint,
        authorization,
        model,
        history,
        timeout_ms,
        context,
        mode,
        options,
        no_proxy,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_observed(
    endpoint: &str,
    authorization: Option<&str>,
    model: &str,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    mode: RequestMode,
    options: &saaa_larm_session::http_api::LlmOptions,
    no_proxy: bool,
    observation: Option<&dyn observation::ObservationSink>,
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
        "stream": mode == RequestMode::Stream,
        "max_tokens": context.max_output_tokens,
    });
    options.apply(
        &mut body,
        model,
        context.max_output_tokens,
        context.reasoning_effort,
    );
    body["stream"] = json!(mode == RequestMode::Stream);
    if mode == RequestMode::Stream && observation.is_some() {
        body["stream_options"] = json!({"include_usage":true});
    }
    let mut client_builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5));
    if no_proxy {
        client_builder = client_builder.no_proxy();
    }
    let client = client_builder
        .build()
        .map_err(|_| ProviderAttemptError::failed(Failure::Internal, false))?;
    let mut attempt = observation::Attempt::new(observation, &body);
    if observation.is_some() && body.to_string().len() > 96_000 {
        let result = Err(ProviderAttemptError::failed(
            Failure::RequestTooLarge,
            false,
        ));
        attempt.finish(&result);
        return result;
    }
    let mut request = client.post(url).json(&body);
    if let Some(authorization) = authorization {
        request = request.header(reqwest::header::AUTHORIZATION, authorization);
    }
    let result = if mode == RequestMode::Stream {
        stream_response::run(request, model, timeout_ms, context, &mut attempt).await
    } else {
        json_response::run(request, timeout_ms, context, &mut attempt).await
    };
    attempt.finish(&result);
    result
}
