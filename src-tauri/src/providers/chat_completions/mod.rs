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
mod world_eval_fixture;
mod world_eval_tests;

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
    let mut first_visible_ms: Option<u64> = None;
    let mut started = false;
    let mut provider_progressed = false;
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
        let (mut calls, mut voice_calls) = (0, 0);
        let mut context_still_calls = 0;
        let mut context_still_call_keys = std::collections::HashSet::new();
        let mut spoken_tool_progress = 0;
        loop {
            crate::runtime::context::world::dispatch::refresh_json(
                &mut messages,
                context.output_persistence.and_then(|p| p.world),
            )
            .map_err(|_| Failure::ContextScopeChanged)?;
            let streaming = mode == RequestMode::Stream && options.streaming;
            let offer = if mode != RequestMode::JsonProbe
                && options.tools
                && !crate::runtime::context::state_answer::is_state_query(&context.input.content)
            {
                available_agent_tools(
                    context.output_persistence,
                    context.input,
                    calls,
                    voice_calls,
                    context_still_calls,
                )
            } else {
                AgentToolOffer::empty()
            };
            let tools = &offer.definitions;
            let (body, generation) = {
                let mut recompose_attempts = 0;
                loop {
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
                    match generation::RequestGeneration::begin(
                        &context,
                        &body,
                        calls,
                        include_world,
                    ) {
                        Ok(generation) => break (body, generation),
                        Err(Failure::RequiredContextOverflow)
                            if calls > 0 && recompose_attempts < 2 =>
                        {
                            recompose_attempts += 1;
                            if !world_body::trim_optional_history_for_tool_follow_up(
                                &mut messages,
                                context.context_sources,
                            ) {
                                return Err(Failure::RequiredContextOverflow);
                            }
                        }
                        Err(error) => return Err(error),
                    }
                }
            };
            let request_id = crate::new_id("provider-request");
            let transport = if streaming { "sse" } else { "json" };
            let audit_url = url::Url::parse(&url)
                .ok()
                .map(|mut parsed| {
                    let _ = parsed.set_username("");
                    let _ = parsed.set_password(None);
                    parsed.set_query(None);
                    parsed.set_fragment(None);
                    parsed.to_string()
                })
                .unwrap_or_else(|| "invalid-endpoint".to_string());
            let record_transport = |stage: &str, detail: Option<&str>| {
                if let Some(persistence) = context.output_persistence {
                    persistence.record_transport_event(
                        &request_id,
                        stage,
                        transport,
                        model,
                        &audit_url,
                        detail,
                    );
                }
            };
            record_transport("prepared", None);
            let mut request = client
                .post(&url)
                .header("X-Request-ID", &context.input.run_id)
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
                let mut value = reqwest::header::HeaderValue::from_str(value)
                    .map_err(|_| Failure::Authentication)?;
                value.set_sensitive(true);
                request = request.header(reqwest::header::AUTHORIZATION, value);
            }
            if let Some(world) = context.output_persistence.and_then(|p| p.world) {
                let sent = world.blocks().is_some_and(|blocks| {
                    body["messages"].as_array().is_some_and(|messages| {
                        messages.iter().any(|m| {
                            m["role"] == "assistant"
                                && m["content"].as_str() == Some(blocks.with_world.as_str())
                        })
                    })
                });
                if sent && !world.revalidate_current() {
                    generation.fail("world-changed-before-send");
                    return Err(Failure::ContextScopeChanged);
                }
            }
            let _ = context.on_event.send(RuntimeEvent::Activity {
                run_id: context.input.run_id.clone(),
                kind: "llm-request-sending".into(),
                summary: "Sending the LLM request and waiting for its response.".into(),
            });
            record_transport("sending", None);
            let response = match super::http::send(request, &context.cancellation, !started).await {
                Ok(response) => response,
                Err(error) => {
                    record_transport("failed", Some(error.as_str()));
                    generation.finish_error(error);
                    return Err(error);
                }
            };
            let response_status = response.status().as_u16().to_string();
            record_transport("headers-received", Some(&response_status));
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
                record_transport("failed", Some("protocol"));
                generation.fail("protocol");
                return Err(Failure::Protocol);
            }
            let mut stream = response.bytes_stream();
            let mut response_started = false;
            let mut prefetched = if !streaming {
                let mut bytes = Vec::new();
                while let Some(chunk) = stream.next().await {
                    let chunk = match chunk {
                        Ok(chunk) => chunk,
                        Err(_) => {
                            record_transport("failed", Some("response-interrupted"));
                            generation.record_usage(
                                None,
                                None,
                                crate::runtime::context::usage::UsageSource::Disconnected,
                                &usage_timings(request_started, first_visible_ms),
                                None,
                            );
                            return Err(Failure::ResponseInterrupted);
                        }
                    };
                    if !response_started {
                        record_transport("response-started", None);
                        response_started = true;
                    }
                    if bytes.len() + chunk.len() > 1_048_576 {
                        record_transport("failed", Some("request-too-large"));
                        return Err(Failure::RequestTooLarge);
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Some(match json_events(&bytes) {
                    Ok(events) => events,
                    Err(error) => {
                        record_transport("failed", Some(error.as_str()));
                        return Err(error);
                    }
                })
            } else {
                None
            };
            let mut decoder = sse::SseDecoder::default();
            let mut completion = chunks::Completion::default();
            while !completion.done {
                let events = if let Some(events) = prefetched.take() {
                    events
                } else {
                    let chunk = match stream.next().await {
                        Some(Ok(chunk)) => chunk,
                        Some(Err(_)) | None => {
                            record_transport("failed", Some("response-interrupted"));
                            generation.record_usage(
                                None,
                                completion.usage.as_ref(),
                                crate::runtime::context::usage::UsageSource::Disconnected,
                                &usage_timings(request_started, first_visible_ms),
                                None,
                            );
                            return Err(Failure::ResponseInterrupted);
                        }
                    };
                    if !response_started {
                        record_transport("response-started", None);
                        response_started = true;
                    }
                    match decoder.push(&chunk) {
                        Ok(events) => events,
                        Err(error) => {
                            record_transport("failed", Some(error.as_str()));
                            return Err(error);
                        }
                    }
                };
                let received_at = Instant::now();
                for data in events {
                    let text = match completion.absorb(&data, model) {
                        Ok(text) => text,
                        Err(error) => {
                            record_transport("failed", Some(error.as_str()));
                            return Err(error);
                        }
                    };
                    provider_progressed |= completion.provider_progressed();
                    if context.cancellation.is_cancelled() {
                        record_transport("failed", Some("cancelled"));
                        return Err(Failure::Cancelled);
                    }
                    if !text.is_empty() {
                        if first_visible_ms.is_none() {
                            super::http_metrics::record(
                                "llmRequestToFirstContent",
                                request_started.elapsed(),
                            );
                            first_visible_ms = Some(
                                u64::try_from(request_started.elapsed().as_millis())
                                    .unwrap_or(u64::MAX),
                            );
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
                            .map_err(|_| {
                                record_transport("failed", Some("client-disconnected"));
                                Failure::ClientDisconnected
                            })?;
                    }
                }
            }
            record_transport("completed", None);
            let tool_calls = completion.complete()?;
            let source = if completion.usage.is_some() {
                crate::runtime::context::usage::UsageSource::Provider
            } else {
                crate::runtime::context::usage::UsageSource::Missing
            };
            generation.record_usage(
                None,
                completion.usage.as_ref(),
                source,
                &usage_timings(request_started, first_visible_ms),
                remember_wire_prefix(
                    context.output_persistence,
                    &context.input.conversation_id,
                    &serde_json::to_vec(&body).unwrap_or_default(),
                ),
            );
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
                generation.revalidate_before_tool()?;
                // Prevent transport/routing retries after a tool has executed, even with no visible text.
                mark_started(&context, &mut started)?;
                calls += 1;
                if call.name == crate::voice_behavior::UPDATE_VOICE_BEHAVIOR_TOOL_NAME {
                    voice_calls += 1;
                }
                let duplicate_context_still_call =
                    crate::runtime::agent_tools::context_still_call_key(&call)
                        .is_some_and(|key| !context_still_call_keys.insert(key));
                if crate::runtime::agent_tools::is_context_still_tool(&call.name) {
                    context_still_calls += 1;
                }
                let report_progress = context.on_event.voice_response_enabled()
                    && spoken_tool_progress < 4
                    && voice_progress::supports(&call.name);
                // The tool budget is the turn's actual remaining time, never a fixed constant.
                let (result, progress_spoken) = if duplicate_context_still_call {
                    (
                        crate::runtime::agent_tools::tool_error_content(
                            "duplicate-memory-recall",
                            "The same typed memory query was already completed in this turn.",
                        ),
                        false,
                    )
                } else {
                    voice_progress::execute(
                        &context,
                        &call,
                        report_progress,
                        Duration::from_millis(timeout_ms).saturating_sub(request_started.elapsed()),
                        &offer.generated,
                    )
                    .await
                };
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
        let kind = if provider_progressed
            && !started
            && matches!(
                kind,
                Failure::Unavailable
                    | Failure::Upstream
                    | Failure::Network
                    | Failure::Timeout
                    | Failure::AllocationLost
            ) {
            Failure::PartialOutput
        } else {
            kind
        };
        if kind == Failure::Cancelled {
            Error::Cancelled {
                output_started: started,
            }
        } else {
            Error::failed(kind, started)
        }
    })
}

mod response_helpers;
use response_helpers::{json_events, mark_started};

fn usage_timings(
    request_started: Instant,
    first_visible_ms: Option<u64>,
) -> crate::runtime::context::usage::UsageTimings {
    crate::runtime::context::usage::UsageTimings {
        ttft_ms: first_visible_ms,
        first_visible_ms,
        completed_ms: Some(
            u64::try_from(request_started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ),
    }
}

fn remember_wire_prefix(
    persistence: Option<crate::ProviderOutputPersistence<'_>>,
    conversation_id: &str,
    wire: &[u8],
) -> Option<i64> {
    let persistence = persistence?;
    let mut prefixes = persistence.state.wire_prefixes.lock().ok()?;
    let previous = prefixes
        .iter()
        .rev()
        .find(|(id, _)| id == conversation_id)
        .map(|(_, bytes)| crate::runtime::context::segment::prefix_match_bytes(bytes, wire) as i64);
    prefixes.retain(|(id, _)| id != conversation_id);
    prefixes.push_back((conversation_id.to_string(), wire.to_vec()));
    while prefixes.len() > 4 {
        prefixes.pop_front();
    }
    previous
}

mod world_claim_tests;
