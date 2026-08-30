use futures_util::StreamExt;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

use super::completion::{
    thinking_enabled, validate_non_stream_completion, CompletionFinish, CompletionTerminal,
    CompletionTerminalError,
};
use super::openai_compatible::{
    drain_sse_events, provider_api_key, provider_chat_url, sse_event_data, SseDrainError,
};
use crate::ipc_contract::{ConversationMessage, RuntimeEvent};
use crate::{OpenAiCompatibleProviderSettings, RunCancellation, StartTurnInput};

mod attempt;
mod dispatch;
mod dynamic_lan;
mod finalize;
mod larm;

pub(crate) use attempt::*;
pub(crate) use dispatch::*;
pub(crate) use dynamic_lan::*;
use finalize::finalize_stream_completion;
pub(crate) use larm::*;

pub(crate) struct ModelStreamContext<'a> {
    pub(crate) reasoning_effort: &'a str,
    pub(crate) max_output_tokens: u32,
    pub(crate) input: &'a StartTurnInput,
    pub(crate) on_event: &'a tauri::ipc::Channel<RuntimeEvent>,
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
    require_event_stream: bool,
    context: ModelStreamContext<'_>,
) -> Result<String, ProviderAttemptError> {
    if context.cancellation.is_cancelled() {
        return Err(ProviderAttemptError::Cancelled {
            output_started: false,
        });
    }
    let mut client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .redirect(reqwest::redirect::Policy::none());
    if provider.location == "local" {
        client = client.no_proxy();
    }
    let client = client
        .build()
        .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Internal, false))?;
    let mut messages = history
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
    let mut tool_calls_this_attempt = 0_usize;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let round_timeout = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| ProviderAttemptError::failed(ProviderFailureKind::Timeout, false))?;
        let tools = available_agent_tools(
            context.output_persistence,
            context.input,
            tool_calls_this_attempt,
        );
        match stream_model_provider_round(
            &client,
            provider,
            &messages,
            &tools,
            context.reasoning_effort,
            context.max_output_tokens,
            api_key,
            context.input,
            context.on_event,
            context.cancellation.clone(),
            context.output_persistence,
            round_timeout,
            require_event_stream,
        )
        .await?
        {
            ModelProviderCompletion::Content(content) => return Ok(content),
            ModelProviderCompletion::ToolCall(call) => {
                if !tool_was_offered(&tools, &call.name) {
                    return Err(ProviderAttemptError::failed(
                        ProviderFailureKind::Protocol,
                        false,
                    ));
                }
                tool_calls_this_attempt += 1;
                let tool_timeout = deadline
                    .checked_duration_since(tokio::time::Instant::now())
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or_else(|| {
                        ProviderAttemptError::failed(ProviderFailureKind::Timeout, false)
                    })?;
                let tool_content = tokio::select! {
                    _ = context.cancellation.cancelled() => {
                        return Err(ProviderAttemptError::Cancelled { output_started: false });
                    }
                    content = execute_agent_tool(
                        context.output_persistence,
                        context.input,
                        &call,
                        tool_timeout,
                    ) => content,
                };
                crate::runtime::agent_tools::append_tool_exchange(
                    &mut messages,
                    &call,
                    tool_content,
                );
            }
        }
    }
}

pub(crate) enum ModelProviderCompletion {
    Content(String),
    ToolCall(crate::runtime::agent_tools::AgentToolCall),
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn stream_model_provider_round(
    client: &reqwest::Client,
    provider: &OpenAiCompatibleProviderSettings,
    messages: &[Value],
    tools: &[Value],
    reasoning_effort: &str,
    max_output_tokens: u32,
    api_key: Option<&str>,
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    cancellation: Arc<RunCancellation>,
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    round_timeout: Duration,
    require_event_stream: bool,
) -> Result<ModelProviderCompletion, ProviderAttemptError> {
    let mut body = json!({
        "model": provider.model,
        "messages": messages,
        "stream": true,
        "reasoning_effort": reasoning_effort,
        "max_tokens": max_output_tokens
    });
    if provider.location == "local" {
        body["chat_template_kwargs"] =
            json!({ "enable_thinking": thinking_enabled(reasoning_effort) });
    }
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = json!("auto");
    }
    let mut request = client
        .post(
            provider_chat_url(&provider.endpoint)
                .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Contract, false))?,
        )
        .timeout(round_timeout)
        .json(&body);
    let configured_api_key = provider_api_key(provider);
    if let Some(api_key) = api_key.or(configured_api_key.as_deref()) {
        request = request.bearer_auth(api_key);
    }
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(ProviderAttemptError::Cancelled { output_started: false }),
        response = request.send() => response.map_err(|error| {
            ProviderAttemptError::failed(classify_reqwest_error(&error), false)
        })?,
    };
    if !response.status().is_success() {
        return Err(ProviderAttemptError::failed(
            classify_provider_status(response.status()),
            false,
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if require_event_stream && !content_type.starts_with("text/event-stream") {
        return Err(ProviderAttemptError::failed(
            ProviderFailureKind::Protocol,
            false,
        ));
    }
    if content_type.contains("application/json") {
        let body = read_provider_body_limited(response, 1_048_576, &cancellation, false).await?;
        let response: Value = serde_json::from_slice(&body)
            .map_err(|_| ProviderAttemptError::failed(ProviderFailureKind::Protocol, false))?;
        let finish = validate_non_stream_completion(&response)
            .map_err(|error| completion_terminal_failure(error, false))?;
        let tool_call = crate::runtime::agent_tools::parse_non_stream_tool_call(&response)
            .map_err(|error| tool_protocol_failure(error, false))?;
        let content = response
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str);
        if let Some(call) = tool_call {
            if finish != CompletionFinish::ToolCalls
                || content.is_some_and(|value| !value.trim().is_empty())
            {
                return Err(ProviderAttemptError::failed(
                    ProviderFailureKind::Protocol,
                    false,
                ));
            }
            return Ok(ModelProviderCompletion::ToolCall(call));
        }
        if finish != CompletionFinish::Stop {
            return Err(ProviderAttemptError::failed(
                ProviderFailureKind::Protocol,
                false,
            ));
        }
        let content = content
            .ok_or_else(|| ProviderAttemptError::failed(ProviderFailureKind::Protocol, false))?;
        if content.chars().count() > 64_000 {
            return Err(ProviderAttemptError::failed(
                ProviderFailureKind::RequestTooLarge,
                false,
            ));
        }
        let content = content.to_string();
        if content.trim().is_empty() {
            return Err(ProviderAttemptError::failed(
                ProviderFailureKind::Protocol,
                false,
            ));
        }
        if let Some(persistence) = output_persistence {
            persistence.mark_started()?;
        }
        on_event
            .send(RuntimeEvent::Delta {
                run_id: input.run_id.clone(),
                text: content.clone(),
            })
            .map_err(|_| {
                ProviderAttemptError::failed(ProviderFailureKind::ClientDisconnected, true)
            })?;
        return Ok(ModelProviderCompletion::Content(content));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();
    let mut content = String::new();
    let mut content_chars = 0_usize;
    let mut output_started = false;
    let mut stream_completed = false;
    let mut terminal = CompletionTerminal::default();
    let mut tool_calls = crate::runtime::agent_tools::ToolCallAccumulator::default();
    loop {
        if cancellation.is_cancelled() {
            return Err(ProviderAttemptError::Cancelled { output_started });
        }
        let next = tokio::select! {
            _ = cancellation.cancelled() => return Err(ProviderAttemptError::Cancelled { output_started }),
            next = stream.next() => next,
        };
        let Some(chunk) = next else {
            if !buffer.is_empty() {
                buffer.extend_from_slice(b"\n\n");
                stream_completed = project_sse_events(
                    drain_sse_events(&mut buffer, 1_048_576)
                        .map_err(|error| sse_drain_failure(error, output_started))?,
                    &mut content,
                    &mut content_chars,
                    &mut output_started,
                    input,
                    on_event,
                    output_persistence,
                    &mut tool_calls,
                    &mut terminal,
                )?;
            }
            break;
        };
        let chunk = chunk.map_err(|error| {
            ProviderAttemptError::failed(classify_reqwest_error(&error), output_started)
        })?;
        buffer.extend_from_slice(&chunk);
        let events = drain_sse_events(&mut buffer, 1_048_576)
            .map_err(|error| sse_drain_failure(error, output_started))?;
        if buffer.len() > 1_048_576 {
            return Err(ProviderAttemptError::failed(
                ProviderFailureKind::RequestTooLarge,
                output_started,
            ));
        }
        let stream_done = project_sse_events(
            events,
            &mut content,
            &mut content_chars,
            &mut output_started,
            input,
            on_event,
            output_persistence,
            &mut tool_calls,
            &mut terminal,
        )?;
        if stream_done {
            stream_completed = true;
            break;
        }
    }
    if !stream_completed {
        return Err(ProviderAttemptError::failed(
            ProviderFailureKind::Network,
            output_started,
        ));
    }
    let tool_call = tool_calls
        .finish()
        .map_err(|error| tool_protocol_failure(error, output_started))?;
    let finish = terminal
        .complete()
        .map_err(|error| completion_terminal_failure(error, output_started))?;
    finalize_stream_completion(content, tool_call, finish, output_started)
}

pub(crate) fn sse_drain_failure(
    error: SseDrainError,
    output_started: bool,
) -> ProviderAttemptError {
    let kind = match error {
        SseDrainError::InvalidUtf8 => ProviderFailureKind::Protocol,
        SseDrainError::EventTooLarge => ProviderFailureKind::RequestTooLarge,
    };
    ProviderAttemptError::failed(kind, output_started)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn project_sse_events(
    events: Vec<String>,
    content: &mut String,
    content_chars: &mut usize,
    output_started: &mut bool,
    input: &StartTurnInput,
    on_event: &tauri::ipc::Channel<RuntimeEvent>,
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    tool_calls: &mut crate::runtime::agent_tools::ToolCallAccumulator,
    terminal: &mut CompletionTerminal,
) -> Result<bool, ProviderAttemptError> {
    for event in events {
        let Some(data) = sse_event_data(&event) else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            terminal
                .complete()
                .map_err(|error| completion_terminal_failure(error, *output_started))?;
            return Ok(true);
        }
        let value: Value = serde_json::from_str(data).map_err(|_| {
            ProviderAttemptError::failed(ProviderFailureKind::Protocol, *output_started)
        })?;
        terminal
            .observe(&value)
            .map_err(|error| completion_terminal_failure(error, *output_started))?;
        tool_calls
            .absorb_stream_delta(&value)
            .map_err(|error| tool_protocol_failure(error, *output_started))?;
        if let Some(delta) = value
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            let delta_chars = delta.chars().count();
            if delta_chars == 0 {
                continue;
            }
            let remaining = 64_000usize.saturating_sub(*content_chars);
            if remaining == 0 || delta_chars > remaining {
                return Err(ProviderAttemptError::failed(
                    ProviderFailureKind::RequestTooLarge,
                    *output_started,
                ));
            }
            if !*output_started {
                if let Some(persistence) = output_persistence {
                    persistence.mark_started()?;
                }
            }
            content.push_str(delta);
            *content_chars += delta_chars;
            *output_started = true;
            on_event
                .send(RuntimeEvent::Delta {
                    run_id: input.run_id.clone(),
                    text: delta.to_string(),
                })
                .map_err(|_| {
                    ProviderAttemptError::failed(ProviderFailureKind::ClientDisconnected, true)
                })?;
        }
    }
    Ok(false)
}

fn completion_terminal_failure(
    error: CompletionTerminalError,
    output_started: bool,
) -> ProviderAttemptError {
    let kind = match error {
        CompletionTerminalError::PartialOutput => ProviderFailureKind::PartialOutput,
        CompletionTerminalError::Policy => ProviderFailureKind::Policy,
        CompletionTerminalError::Protocol => ProviderFailureKind::Protocol,
    };
    ProviderAttemptError::failed(kind, output_started)
}
