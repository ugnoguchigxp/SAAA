//! OpenAI Chat Completions data plane. Bootstrap/leases remain outside this client.
use super::stream::{
    available_agent_tools, tool_was_offered, AgentToolOffer, ModelStreamContext,
    ProviderAttemptError as Error, ProviderFailureKind as Failure,
};
use crate::ipc_contract::{ConversationMessage, RuntimeEvent};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

mod chunks;
mod generation;
mod sse;
mod tests;
mod voice_progress;
mod world_body;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestMode {
    Stream,
    JsonProbe,
    #[cfg(test)]
    JsonTools,
}

mod test_helpers;
#[cfg(test)]
pub(crate) use test_helpers::{run, run_mode};

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
) -> Result<String, Error> {
    let request_started = Instant::now();
    let mut first_content = true;
    let mut started = false;
    let mut messages = world_body::build_messages(history, &context);
    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        let url = super::openai_compatible::provider_operation_url(endpoint, "chat/completions")
            .map_err(|_| Failure::Contract)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| Failure::Internal)?;
        let mut output = String::new();
        let mut calls = 0;
        let mut voice_calls = 0;
        let mut spoken_tool_progress = 0;
        loop {
            let streaming = mode == RequestMode::Stream && options.streaming;
            let offer = if mode != RequestMode::JsonProbe && options.tools {
                available_agent_tools(
                    context.output_persistence,
                    context.input,
                    calls,
                    voice_calls,
                )
            } else {
                AgentToolOffer::empty()
            };
            let tools = &offer.definitions;
            let world = context
                .output_persistence
                .and_then(|persistence| persistence.world);
            let include_world = world_body::apply(&mut messages, world);
            let mut body = json!({"model": model, "messages": messages, "stream": streaming,
                "max_tokens": context.max_output_tokens});
            options.apply(
                &mut body,
                model,
                context.max_output_tokens,
                context.reasoning_effort,
            );
            if !tools.is_empty() {
                body["tools"] = json!(tools);
                body["parallel_tool_calls"] = json!(false);
            }
            let generation =
                generation::RequestGeneration::begin(&context, &body, calls, include_world)?;
            let mut request = client
                .post(&url)
                .header(
                    "Accept",
                    if streaming {
                        "text/event-stream"
                    } else {
                        "application/json"
                    },
                )
                .json(&body);
            if let Some(value) = authorization {
                request = request.header("Authorization", value);
            }
            let response = match super::http::send(request, &context.cancellation, !started).await {
                Ok(response) => response,
                Err(error) => {
                    generation.finish_error(error);
                    return Err(error);
                }
            };
            if response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                .map(str::trim)
                != Some(if streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                })
            {
                generation.fail("protocol");
                return Err(Failure::Protocol);
            }
            let mut stream = response.bytes_stream();
            let mut prefetched = if !streaming {
                let mut bytes = Vec::new();
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.map_err(|_| Failure::Network)?;
                    if bytes.len() + chunk.len() > 1_048_576 {
                        return Err(Failure::RequestTooLarge);
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Some(json_events(&bytes)?)
            } else {
                None
            };
            let mut decoder = sse::SseDecoder::default();
            let mut completion = chunks::Completion::default();
            while !completion.done {
                let events = if let Some(events) = prefetched.take() {
                    events
                } else {
                    let chunk = stream
                        .next()
                        .await
                        .ok_or(Failure::Network)?
                        .map_err(|_| Failure::Network)?;
                    decoder.push(&chunk)?
                };
                let received_at = Instant::now();
                for data in events {
                    let text = completion.absorb(&data, model)?;
                    if context.cancellation.is_cancelled() {
                        return Err(Failure::Cancelled);
                    }
                    if !text.is_empty() {
                        if first_content {
                            super::http_metrics::record(
                                "llmRequestToFirstContent",
                                request_started.elapsed(),
                            );
                            first_content = false;
                        }
                        mark_started(&context, &mut started)?;
                        context
                            .on_event
                            .send_received(
                                RuntimeEvent::Delta {
                                    run_id: context.input.run_id.clone(),
                                    text,
                                },
                                received_at,
                            )
                            .map_err(|_| Failure::ClientDisconnected)?;
                    }
                }
            }
            let tool_calls = completion.complete()?;
            output.push_str(&completion.content);
            if output.len() > 1_048_576 {
                return Err(Failure::RequestTooLarge);
            }
            if tool_calls.is_empty() {
                generation.complete()?;
                return Ok(output);
            }
            if calls + tool_calls.len() > 32 {
                return Err(Failure::Protocol);
            }
            // Validate the entire batch before any tool can produce a side effect.
            if tool_calls
                .iter()
                .any(|call| !tool_was_offered(tools, &call.name))
            {
                generation.fail("protocol");
                return Err(Failure::Protocol);
            }
            generation.complete()?;
            messages.push(json!({"role": "assistant", "content": completion.content,
                "tool_calls": tool_calls.iter().map(|call| json!({"id": call.id, "type": "function",
                    "function": {"name": call.name, "arguments": call.arguments}})).collect::<Vec<_>>()}));
            for call in tool_calls {
                if context.cancellation.is_cancelled() {
                    return Err(Failure::Cancelled);
                }
                if !tool_was_offered(tools, &call.name) {
                    return Err(Failure::Protocol);
                }
                // Prevent transport/routing retries after a tool has executed, even with no visible text.
                mark_started(&context, &mut started)?;
                calls += 1;
                if call.name == crate::voice_behavior::UPDATE_VOICE_BEHAVIOR_TOOL_NAME {
                    voice_calls += 1;
                }
                let report_progress = context.on_event.voice_response_enabled()
                    && spoken_tool_progress < voice_progress::MAX_SPOKEN_PER_ATTEMPT
                    && voice_progress::supports(&call.name);
                // The tool budget is the turn's actual remaining time, never a fixed constant.
                let (result, progress_spoken) = voice_progress::execute(
                    &context,
                    &call,
                    report_progress,
                    Duration::from_millis(timeout_ms).saturating_sub(request_started.elapsed()),
                    &offer.generated,
                )
                .await;
                if progress_spoken {
                    spoken_tool_progress += 1;
                }
                if result.len() > 262_144 {
                    return Err(Failure::RequestTooLarge);
                }
                if crate::generative_ui::tools::NAMES.contains(&call.name.as_str())
                    && serde_json::from_str::<Value>(&result)
                        .ok()
                        .is_some_and(|v| v["messageId"].is_string())
                {
                    let _ = context.on_event.send(RuntimeEvent::Activity {
                        run_id: context.input.run_id.clone(),
                        kind: "ui-presented".into(),
                        summary: "An inline view is available.".into(),
                    });
                }
                messages.push(json!({"role": "tool", "tool_call_id": call.id, "content": result}));
            }
        }
    });
    let result = tokio::select! {
        biased;
        _ = context.cancellation.cancelled() => Err(Failure::Cancelled),
        result = result => result.unwrap_or(Err(Failure::Timeout)),
    };
    result.map_err(|kind| {
        if kind == Failure::Cancelled {
            Error::Cancelled {
                output_started: started,
            }
        } else {
            Error::failed(kind, started)
        }
    })
}

fn mark_started(context: &ModelStreamContext<'_>, started: &mut bool) -> Result<(), Failure> {
    if !*started {
        if let Some(persistence) = context.output_persistence {
            persistence.mark_started().map_err(|_| Failure::Internal)?;
        }
        *started = true;
    }
    Ok(())
}

fn json_events(bytes: &[u8]) -> Result<Vec<String>, Failure> {
    let mut value: Value = serde_json::from_slice(bytes).map_err(|_| Failure::Protocol)?;
    if value.get("error").is_some() {
        return Err(Failure::Upstream);
    }
    let choices = value
        .get_mut("choices")
        .and_then(Value::as_array_mut)
        .ok_or(Failure::Protocol)?;
    if choices.len() != 1 {
        return Err(Failure::Protocol);
    }
    let choice = choices[0].as_object_mut().ok_or(Failure::Protocol)?;
    let mut message = choice
        .remove("message")
        .filter(Value::is_object)
        .ok_or(Failure::Protocol)?;
    if let Some(calls) = message.get_mut("tool_calls").filter(|v| !v.is_null()) {
        for (index, call) in calls
            .as_array_mut()
            .ok_or(Failure::Protocol)?
            .iter_mut()
            .enumerate()
        {
            call.as_object_mut()
                .ok_or(Failure::Protocol)?
                .insert("index".into(), json!(index));
        }
    }
    choice.insert("delta".into(), message);
    Ok(vec![value.to_string(), "[DONE]".to_string()])
}
