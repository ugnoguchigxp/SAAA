const MAX_CONTENT_BYTES: usize = 262_144;
const MAX_CONTENT_CHARS: usize = 64_000;
#[cfg(test)]
use transport::{accept_event, parse_event, take_event};
#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn capability_wrappers_have_a_measurable_wire_reservation() {
        let base = render_turn_input(&[]).expect("empty conversation serializes");
        let prepared = decorate_turn_input(
            &base,
            "<saaa-ui-00000000000000000000000000000000>",
            true,
            true,
            Value::Null,
        );
        let base_bytes = serde_json::to_vec(&generation::turn_request_body(&base)).unwrap();
        let prepared_bytes = serde_json::to_vec(&generation::turn_request_body(&prepared)).unwrap();
        assert!(prepared_bytes.len() > base_bytes.len());
        assert!(prepared.contains("codingTools"));
        assert!(prepared.contains("delegatedWorkTools"));
    }
}
const MAX_SSE_EVENT_BYTES: usize = 1_048_576;
const MAX_RECONNECTS: usize = 3;
/// Calculates the bytes added to an AgentSession turn by its live capability wrappers. The base
/// conversation is intentionally empty: the wrappers parse and reserialize it unchanged, so the
/// serialized request-body delta is independent of history content while retaining the actual
/// UI/coding definitions and coding context for this conversation.
pub(super) fn initial_input_reserve(
    state: &crate::AppState,
    input: &crate::StartTurnInput,
) -> Result<usize, String> {
    let base = render_turn_input(&[]).map_err(|kind| kind.as_str().to_string())?;
    let (enabled, coding_enabled) = live_capability_flags(state);
    let coding_context = if coding_enabled {
        coding_context(state, input)
    } else {
        Value::Null
    };
    let prepared = decorate_turn_input(
        &base,
        "<saaa-ui-00000000000000000000000000000000>",
        enabled,
        coding_enabled,
        coding_context,
    );
    let base_bytes = serde_json::to_vec(&generation::turn_request_body(&base))
        .map_err(|error| format!("could not serialize AgentSession base input: {error}"))?
        .len();
    let prepared_bytes = serde_json::to_vec(&generation::turn_request_body(&prepared))
        .map_err(|error| format!("could not serialize AgentSession capability input: {error}"))?
        .len();
    Ok(prepared_bytes.saturating_sub(base_bytes))
}
fn live_capability_flags(state: &crate::AppState) -> (bool, bool) {
    let enabled = state
        .sqlite_readers
        .read(crate::generative_ui::store::enabled)
        .unwrap_or(false);
    let coding_enabled = state
        .sqlite_readers
        .read(crate::coding::repository::enabled)
        .unwrap_or(false);
    (enabled, coding_enabled)
}
fn coding_context(state: &crate::AppState, input: &crate::StartTurnInput) -> Value {
    crate::coding::tools::context(state, &input.conversation_id)
}
fn decorate_turn_input(
    base: &str,
    marker: &str,
    enabled: bool,
    coding_enabled: bool,
    coding_context: Value,
) -> String {
    let mut result = base.to_string();
    if !coding_enabled {
        result = json!({"type":"saaa.conversation.capabilities.v1","conversation":serde_json::from_str::<Value>(&result).unwrap_or(Value::Null),"instructions":"SAAA coding tools are disabled on this route. You cannot start, resume, inspect or cancel a local coding job. Explain this limitation for coding requests and ask the user to enable pi coding in SAAA settings. Never claim to have performed an implementation or use your own remote tools as a substitute."}).to_string();
    }
    if enabled {
        result = ui_bridge::initial_input(&result, marker);
    }
    if coding_enabled {
        result = ui_bridge::coding_input(&result, marker, coding_context);
    }
    result
}
#[derive(Debug, Deserialize)]
struct TurnResponse {
    id: String,
    #[serde(default)]
    session_id: Option<String>,
}
#[derive(Debug, Deserialize)]
struct AgentEvent {
    #[serde(rename = "type")]
    event_type: String,
    session_id: String,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    data: Value,
}
struct ParsedEvent {
    event_name: Option<String>,
    id: Option<String>,
    payload: AgentEvent,
}
#[derive(Default)]
struct StreamState {
    content: String,
    content_chars: usize,
    output_started: bool,
    last_cursor: Option<String>,
    projection: ui_bridge::Projection,
}
enum ReadResult {
    Terminal(ProviderAttemptOutcome),
    Reconnect,
    Cancelled,
    Failed(ProviderFailureKind),
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_agent_session_sse(
    client: &Client,
    provider: &AgentSessionProviderSettings,
    session: &SessionResponse,
    events_url: Url,
    history: &[ConversationMessage],
    timeout_ms: u64,
    api_key: Option<&str>,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    if context.cancellation.is_cancelled() {
        return super::cancelled(false);
    }
    let deadline = TokioInstant::now() + Duration::from_millis(timeout_ms);
    // This runs after creation and transport negotiation, immediately before the first remote
    // turn. It is the AgentSession equivalent of the OpenAI adapter's send-body revalidation.
    let world = context
        .output_persistence
        .and_then(|persistence| persistence.world);
    let mut current_history = history.to_vec();
    if let Some(world) = world {
        if world.refresh_history(&mut current_history).is_err() {
            return failed(ProviderFailureKind::ContextScopeChanged, false);
        }
    }
    let history = current_history.as_slice();
    let (provider_history, initial_world) = world
        .map(|world| world.provider_history(history))
        .unwrap_or_else(|| (history.to_vec(), false));
    // AgentSession represents tool continuation by wrapping the original input. Keep a distinct
    // no-World base so a current first-turn snapshot cannot be replayed as a later state claim.
    let follow_up_history = world
        .map(|world| world.without_world_history(history))
        .unwrap_or_else(|| history.to_vec());
    let mut input = match render_turn_input(&provider_history) {
        Ok(input) => input,
        Err(kind) => return failed(kind, false),
    };
    let mut follow_up_base = match render_turn_input(&follow_up_history) {
        Ok(input) => input,
        Err(kind) => return failed(kind, false),
    };
    let (mut enabled, mut coding_enabled) = context
        .output_persistence
        .map(|persistence| live_capability_flags(persistence.state))
        .unwrap_or((false, false));
    if crate::runtime::context::state_answer::is_state_query(&context.input.content) {
        enabled = false;
        coding_enabled = false;
    }
    let mut offered_tools = Vec::new();
    if enabled {
        offered_tools.extend(crate::generative_ui::tools::definitions());
    }
    if coding_enabled {
        offered_tools.extend(crate::coding::tools::definitions());
        offered_tools.extend(crate::steward::tools::definitions());
    }
    let marker = format!("<saaa-ui-{}>", uuid::Uuid::new_v4().simple());
    let initial_coding_context = if coding_enabled {
        context
            .output_persistence
            .map(|persistence| coding_context(persistence.state, context.input))
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    input = decorate_turn_input(
        &input,
        &marker,
        enabled,
        coding_enabled,
        initial_coding_context.clone(),
    );
    follow_up_base = decorate_turn_input(
        &follow_up_base,
        &marker,
        enabled,
        coding_enabled,
        initial_coding_context,
    );
    if input.len() > 1_000_000 || follow_up_base.len() > 1_000_000 {
        return failed(ProviderFailureKind::RequestTooLarge, false);
    }
    let base_envelope = generation::Envelope::new(&follow_up_base);
    let mut cursor = None;
    let mut fresh = None;
    let mut tool_history = Vec::<Value>::new();
    let mut output_started = false;
    for round in 0..=12 {
        // The remote session receives the World only in its initial turn. Tool follow-ups are
        // explicitly recorded without it; they must not claim that an old frame was resent.
        let mut include_world =
            initial_world && round == 0 && world.is_some_and(|world| world.revalidate_current());
        if round > 0 {
            if let Some(world) = world {
                match world.refresh_history(&mut current_history) {
                    Ok(true) => {
                        fresh = match fresh_session::FreshSession::create(
                            client,
                            provider,
                            api_key,
                            context.cancellation.clone(),
                            deadline,
                        )
                        .await
                        {
                            Ok(session) => Some(session),
                            Err(kind) => return failed(kind, output_started),
                        };
                        // Creation may have taken longer than the frame TTL.
                        if world.refresh_history(&mut current_history).is_err() {
                            return failed(
                                ProviderFailureKind::ContextScopeChanged,
                                output_started,
                            );
                        }
                        let base = match render_turn_input(&current_history) {
                            Ok(base) => base,
                            Err(kind) => return failed(kind, output_started),
                        };
                        let base = decorate_turn_input(
                            &base,
                            &marker,
                            enabled,
                            coding_enabled,
                            context
                                .output_persistence
                                .map(|p| coding_context(p.state, context.input))
                                .unwrap_or(Value::Null),
                        );
                        input = generation::Envelope::new(&base).follow_up(
                            &json!({"history":tool_history,"authority":"none"}).to_string(),
                        );
                        include_world = world.revalidate_current();
                        if !include_world {
                            return failed(
                                ProviderFailureKind::ContextScopeChanged,
                                output_started,
                            );
                        }
                        cursor = None;
                    }
                    Ok(false) => {}
                    Err(_) => {
                        return failed(ProviderFailureKind::ContextScopeChanged, output_started)
                    }
                }
            }
        }
        let session = fresh.as_ref().map(|f| &f.session).unwrap_or(session);
        let current_events = fresh.as_ref().map(|f| &f.events).unwrap_or(&events_url);
        if round == 0 && !include_world {
            input = follow_up_base.clone();
        }
        let generation = {
            let mut recompose_attempts = 0;
            loop {
                match base_envelope.begin(&context, round, &input, &offered_tools, include_world) {
                    Ok(generation) => break generation,
                    Err(ProviderFailureKind::RequiredContextOverflow)
                        if round > 0 && recompose_attempts < 2 =>
                    {
                        recompose_attempts += 1;
                        if !generation::trim_optional_history_from_follow_up(
                            &mut input,
                            context.context_sources,
                        ) {
                            return failed(
                                ProviderFailureKind::RequiredContextOverflow,
                                output_started,
                            );
                        }
                    }
                    Err(kind) => return failed(kind, output_started),
                }
            }
        };
        let turn = tokio::select! {
            biased;
            _ = context.cancellation.cancelled() => {
                generation.cancel();
                return super::cancelled(output_started)
            },
            _ = tokio::time::sleep_until(deadline) => {
                generation.fail("timeout");
                return failed(ProviderFailureKind::Timeout, output_started)
            },
            result = start_turn(client, provider, session, &input, api_key) => match result {
                Ok(turn) => turn,
                Err(kind) => {
                    generation.fail(kind.as_str());
                    return failed(kind, output_started)
                },
            }
        };
        let mut state = StreamState {
            last_cursor: cursor.take(),
            output_started,
            projection: ui_bridge::Projection::new(
                (enabled || coding_enabled).then(|| marker.clone()),
            ),
            ..Default::default()
        };
        let outcome = read_turn(
            client,
            provider,
            session,
            current_events,
            &turn,
            deadline,
            api_key,
            &context,
            &mut state,
        )
        .await;
        cursor = state.last_cursor;
        output_started = state.output_started;
        if let Some(session) = fresh.take() {
            if let Err(kind) = session.release().await {
                generation.fail("followup-session-release-unconfirmed");
                return failed(kind, true).with_cleanup(CleanupOutcome::ReleaseFailed {
                    kind: kind.as_str(),
                });
            }
        }
        let ProviderAttemptOutcome::Completed { content, cleanup } = outcome else {
            generation.finish_outcome(&outcome);
            return outcome;
        };
        if let Err(kind) = generation.complete() {
            return failed(kind, output_started);
        }
        if !state.projection.is_control() {
            return ProviderAttemptOutcome::Completed { content, cleanup };
        }
        if round == 12 {
            return failed(ProviderFailureKind::Protocol, output_started);
        }
        if context.cancellation.is_cancelled() {
            return super::cancelled(output_started);
        }
        if TokioInstant::now() >= deadline {
            return failed(ProviderFailureKind::Timeout, output_started);
        }
        let decoded = if content
            .trim_start()
            .starts_with(&marker.replace("saaa-ui-", "saaa-coding-"))
            && coding_enabled
        {
            ui_bridge::coding_decode(&content, &marker)
        } else if enabled {
            ui_bridge::decode(&content, &marker)
        } else {
            Err(())
        };
        let result = match decoded {
            Ok(mut call) => {
                call.id = format!("sse-ui-{marker}-{round}");
                if let Err(kind) = generation.revalidate_before_tool() {
                    return failed(kind, output_started);
                }
                let result = if crate::coding::contracts::NAMES.contains(&call.name.as_str()) {
                    crate::coding::tools::execute(
                        context.output_persistence.map(|p| p.state),
                        context.input,
                        &call,
                    )
                } else if crate::steward::tools::NAMES.contains(&call.name.as_str()) {
                    crate::steward::tools::execute(
                        context.output_persistence.map(|p| p.state),
                        context.input,
                        &call,
                    )
                } else {
                    crate::generative_ui::tools::execute(
                        context.output_persistence.map(|p| p.state),
                        context.input,
                        &call,
                    )
                };
                let value = serde_json::from_str::<Value>(&result)
                    .unwrap_or(json!({"error":"Invalid result"}));
                let presented = value["messageId"].is_string();
                if value["accepted"] == true
                    || presented
                    || (call.name == "save_ui" && value.get("error").is_none())
                {
                    // Persisted mutations must prevent fallback from repeating side effects.
                    if !output_started
                        && context
                            .output_persistence
                            .is_some_and(|p| p.mark_started().is_err())
                    {
                        return failed(ProviderFailureKind::Internal, true);
                    }
                    output_started = true;
                    if presented
                        && context
                            .on_event
                            .send(RuntimeEvent::Activity {
                                run_id: context.input.run_id.clone(),
                                kind: "ui-presented".into(),
                                summary: "An inline view is available.".into(),
                            })
                            .is_err()
                    {
                        return failed(ProviderFailureKind::ClientDisconnected, true);
                    }
                }
                json!({"callId":call.id,"name":call.name,"arguments":call.arguments,"result":value})
            }
            Err(()) => return failed(ProviderFailureKind::Protocol, output_started),
        };
        tool_history.push(result.clone());
        input = ui_bridge::result_input(result, &marker, 11 - round);
        if coding_enabled {
            input = ui_bridge::coding_input(&input, &marker, Value::Null);
        }
        input = base_envelope.follow_up(&input);
        if input.len() > 1_000_000 {
            return failed(ProviderFailureKind::RequestTooLarge, output_started);
        }
    }
    failed(ProviderFailureKind::Protocol, output_started)
}
