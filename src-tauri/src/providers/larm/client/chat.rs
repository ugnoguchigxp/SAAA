use futures_util::StreamExt;
use reqwest::header::CONTENT_TYPE;
use serde_json::Value;
use std::future::pending;
use std::time::Duration;

use super::super::contracts::{BoundedIdentifier, ReadyAllocation, SessionFailureKind};
use super::{
    classify_error_response, classify_transport, drain_sse, lease_deadlines, media_type,
    project_sse, should_retry_stream_renew, tool_protocol_error, Cancellation, ChatCompletion,
    ChatMessage, LarmError, LarmHttpClient, ERROR_BODY_LIMIT, GATEWAY_REQUEST_LIMIT,
    SSE_EVENT_LIMIT, VIRTUAL_MODEL,
};
use crate::providers::completion::{
    thinking_enabled, CompletionFinish, CompletionTerminal, DEFAULT_MAX_OUTPUT_TOKENS,
};
use crate::runtime::agent_tools::ToolCallAccumulator;

impl<'a> LarmHttpClient<'a> {
    pub(crate) async fn chat<F>(
        &self,
        allocation: &ReadyAllocation,
        lease_received_at: tokio::time::Instant,
        messages: &[ChatMessage],
        timeout: Duration,
        cancellation: Cancellation<'_>,
        on_delta: F,
    ) -> Result<ChatCompletion, LarmError>
    where
        F: FnMut(&str, bool) -> Result<(), SessionFailureKind>,
    {
        self.chat_with_tools(
            allocation,
            lease_received_at,
            messages,
            &[],
            &[],
            crate::providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            DEFAULT_MAX_OUTPUT_TOKENS,
            timeout,
            cancellation,
            on_delta,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn chat_with_tools<F>(
        &self,
        allocation: &ReadyAllocation,
        lease_received_at: tokio::time::Instant,
        messages: &[ChatMessage],
        tool_exchanges: &[Value],
        tools: &[Value],
        reasoning_effort: &str,
        max_output_tokens: u32,
        timeout: Duration,
        cancellation: Cancellation<'_>,
        mut on_delta: F,
    ) -> Result<ChatCompletion, LarmError>
    where
        F: FnMut(&str, bool) -> Result<(), SessionFailureKind>,
    {
        let mut serialized_messages = serde_json::to_value(messages)
            .map_err(|_| LarmError::new(SessionFailureKind::Internal, false))?
            .as_array()
            .cloned()
            .ok_or_else(|| LarmError::new(SessionFailureKind::Internal, false))?;
        serialized_messages.extend_from_slice(tool_exchanges);
        let mut request = serde_json::json!({
            "model": VIRTUAL_MODEL,
            "messages": serialized_messages,
            "stream": true,
            "reasoning_effort": reasoning_effort,
            "max_tokens": max_output_tokens,
            "chat_template_kwargs": { "enable_thinking": thinking_enabled(reasoning_effort) }
        });
        if !tools.is_empty() {
            request["tools"] = Value::Array(tools.to_vec());
            request["tool_choice"] = Value::String("auto".to_string());
        }
        let body = serde_json::to_vec(&request)
            .map_err(|_| LarmError::new(SessionFailureKind::Internal, false))?;
        if body.len() > GATEWAY_REQUEST_LIMIT {
            return Err(LarmError::new(SessionFailureKind::RequestTooLarge, false));
        }
        let (mut renew_at, mut expires_at) =
            lease_deadlines(lease_received_at, allocation.effective_ttl_seconds);
        if tokio::time::Instant::now() >= expires_at {
            return Err(LarmError::new(SessionFailureKind::AllocationLost, false));
        }
        let response = self
            .send_with_allocation(
                &body,
                allocation.allocation_id.as_str(),
                timeout,
                cancellation,
            )
            .await?;
        if !response.status().is_success() {
            return Err(LarmError::new(
                classify_error_response(response, ERROR_BODY_LIMIT).await,
                false,
            ));
        }
        let request_id = response
            .headers()
            .get("x-request-id")
            .map(|value| {
                value
                    .to_str()
                    .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))
                    .and_then(|value| {
                        BoundedIdentifier::new(value.to_string())
                            .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))
                    })
            })
            .transpose()?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(media_type)
            .ok_or_else(|| LarmError::new(SessionFailureKind::Protocol, false))?;
        if content_type != "text/event-stream" {
            return Err(LarmError::new(SessionFailureKind::Protocol, false));
        }
        if tokio::time::Instant::now() >= expires_at {
            return Err(LarmError::new(SessionFailureKind::AllocationLost, false));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut content = String::new();
        let mut content_chars = 0;
        let mut output_started = false;
        let mut stream_completed = false;
        let mut terminal = CompletionTerminal::default();
        let mut tool_calls = ToolCallAccumulator::default();
        let mut renewed_allocation = allocation.clone();
        let mut renewed_at = lease_received_at;
        let mut renew_retry = 0_u8;
        loop {
            let next = tokio::select! {
                _ = cancellation.cancelled() => return Err(LarmError::new(SessionFailureKind::Cancelled, output_started)),
                _ = tokio::time::sleep_until(expires_at) => {
                    return Err(LarmError::new(SessionFailureKind::AllocationLost, output_started));
                }
                _ = async {
                    match renew_at {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => pending::<()>().await,
                    }
                } => {
                    let renewal = tokio::select! {
                        _ = tokio::time::sleep_until(expires_at) => {
                            return Err(LarmError::new(SessionFailureKind::AllocationLost, output_started));
                        }
                        result = self.renew(&renewed_allocation, cancellation, output_started) => result,
                    };
                    match renewal {
                        Ok(next) => {
                            renewed_allocation = next;
                            renewed_at = tokio::time::Instant::now();
                            renew_retry = 0;
                            (renew_at, expires_at) = lease_deadlines(
                                renewed_at,
                                renewed_allocation.effective_ttl_seconds,
                            );
                        }
                        Err(error) if should_retry_stream_renew(error.kind, renew_retry)
                            && tokio::time::Instant::now() + Duration::from_secs(1) < expires_at => {
                            renew_retry = 1;
                            renew_at = Some(tokio::time::Instant::now() + Duration::from_secs(1));
                        }
                        Err(_) => {
                            renew_at = None;
                        }
                    }
                    continue;
                }
                next = stream.next() => next,
            };
            let Some(chunk) = next else {
                if !buffer.is_empty() {
                    buffer.extend_from_slice(b"\n\n");
                    stream_completed = project_sse(
                        drain_sse(&mut buffer, output_started)?,
                        &mut content,
                        &mut content_chars,
                        &mut output_started,
                        &mut on_delta,
                        &mut tool_calls,
                        &mut terminal,
                    )?;
                }
                break;
            };
            let chunk = chunk
                .map_err(|error| LarmError::new(classify_transport(&error), output_started))?;
            buffer.extend_from_slice(&chunk);
            let events = drain_sse(&mut buffer, output_started)?;
            if buffer.len() > SSE_EVENT_LIMIT {
                return Err(LarmError::new(
                    SessionFailureKind::RequestTooLarge,
                    output_started,
                ));
            }
            if project_sse(
                events,
                &mut content,
                &mut content_chars,
                &mut output_started,
                &mut on_delta,
                &mut tool_calls,
                &mut terminal,
            )? {
                stream_completed = true;
                break;
            }
        }
        if !stream_completed {
            return Err(LarmError::new(SessionFailureKind::Network, output_started));
        }
        let tool_call = tool_calls
            .finish()
            .map_err(|error| tool_protocol_error(error, output_started))?;
        let finish = terminal
            .complete()
            .map_err(|error| super::completion_terminal_error(error, output_started))?;
        if tool_call.is_some()
            && (finish != CompletionFinish::ToolCalls || !content.trim().is_empty())
        {
            return Err(LarmError::new(SessionFailureKind::Protocol, output_started));
        }
        if tool_call.is_none() && finish != CompletionFinish::Stop {
            return Err(LarmError::new(SessionFailureKind::Protocol, output_started));
        }
        if content.trim().is_empty() && tool_call.is_none() {
            return Err(LarmError::new(SessionFailureKind::Protocol, false));
        }
        Ok(ChatCompletion {
            content,
            tool_call,
            request_id,
            renewed_allocation,
            lease_received_at: renewed_at,
        })
    }
}
