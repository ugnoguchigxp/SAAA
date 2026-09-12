use futures_util::StreamExt;
use reqwest::{header, Client, Response};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::time::Instant as TokioInstant;
use url::Url;

use crate::{
    ipc_contract::{ConversationMessage, RuntimeEvent},
    AgentSessionProviderSettings,
};

use super::{authorized, failed, safe_remote_id, send, session_operation_url, SessionResponse};
use crate::providers::{
    llm_websocket::protocol::{MAX_CONTENT_BYTES, MAX_CONTENT_CHARS},
    stream::{CleanupOutcome, ModelStreamContext, ProviderAttemptOutcome, ProviderFailureKind},
};

const MAX_SSE_EVENT_BYTES: usize = 1_048_576;
const MAX_RECONNECTS: usize = 3;

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
}

enum ReadResult {
    Terminal(ProviderAttemptOutcome),
    Reconnect,
    Cancelled,
    Failed(ProviderFailureKind),
}

pub(super) async fn probe_event_stream(
    client: &Client,
    events_url: &Url,
    api_key: Option<&str>,
) -> Result<(), String> {
    let response = authorized(
        client
            .get(events_url.clone())
            .header(header::ACCEPT, "text/event-stream"),
        api_key,
    )
    .send()
    .await
    .map_err(|error| format!("Connection failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Provider returned HTTP {}", response.status()));
    }
    if !is_event_stream(&response) {
        return Err("Agent Session events endpoint did not return text/event-stream".to_string());
    }
    Ok(())
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
    let turn = match start_turn(client, provider, session, history, api_key).await {
        Ok(turn) => turn,
        Err(ProviderFailureKind::Cancelled) => return super::cancelled(false),
        Err(kind) => return failed(kind, false),
    };
    let deadline = TokioInstant::now() + Duration::from_millis(timeout_ms);
    let mut state = StreamState::default();
    let mut reconnects = 0;
    let result = loop {
        let request = authorized(
            client
                .get(events_url.clone())
                .header(header::ACCEPT, "text/event-stream"),
            api_key,
        );
        let request = match state.last_cursor.as_deref() {
            Some(cursor) => request.header("Last-Event-ID", cursor),
            None => request,
        };
        let response = tokio::select! {
            _ = context.cancellation.cancelled() => break ReadResult::Cancelled,
            _ = tokio::time::sleep_until(deadline) => break ReadResult::Failed(ProviderFailureKind::Timeout),
            response = send(request) => match response {
                Ok(response) => response,
                Err(kind) => break ReadResult::Failed(kind),
            },
        };
        if !is_event_stream(&response) {
            break ReadResult::Failed(ProviderFailureKind::Protocol);
        }
        match read_connection(response, session, &turn.id, deadline, &context, &mut state).await {
            ReadResult::Reconnect if reconnects < MAX_RECONNECTS && state.last_cursor.is_some() => {
                reconnects += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            result => break result,
        }
    };
    let (attempt, terminal) = match result {
        ReadResult::Terminal(outcome) => (outcome, true),
        ReadResult::Cancelled => (super::cancelled(state.output_started), false),
        ReadResult::Failed(kind) => (failed(kind, state.output_started), false),
        ReadResult::Reconnect => (
            failed(ProviderFailureKind::Network, state.output_started),
            false,
        ),
    };
    if !terminal {
        let _ = cancel_turn(client, provider, &session.id, &turn.id, api_key).await;
    }
    attempt
}

async fn start_turn(
    client: &Client,
    provider: &AgentSessionProviderSettings,
    session: &SessionResponse,
    history: &[ConversationMessage],
    api_key: Option<&str>,
) -> Result<TurnResponse, ProviderFailureKind> {
    let input = render_turn_input(history)?;
    let request = authorized(
        client.post(session_operation_url(provider, &session.id, "turns")?),
        api_key,
    )
    .header("Idempotency-Key", idempotency_key())
    .json(&json!({ "input": [{ "type": "text", "text": input }] }));
    let turn: TurnResponse = super::read_json_response(send(request).await?).await?;
    if !safe_remote_id(&turn.id)
        || turn
            .session_id
            .as_deref()
            .is_some_and(|session_id| session_id != session.id)
    {
        return Err(ProviderFailureKind::Protocol);
    }
    Ok(turn)
}

fn render_turn_input(history: &[ConversationMessage]) -> Result<String, ProviderFailureKind> {
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
    let input = serde_json::to_string(&json!({
        "type": "saaa.conversation.v1",
        "messages": messages,
    }))
    .map_err(|_| ProviderFailureKind::Internal)?;
    if input.is_empty() || input.len() > 1_000_000 {
        return Err(ProviderFailureKind::RequestTooLarge);
    }
    Ok(input)
}

async fn cancel_turn(
    client: &Client,
    provider: &AgentSessionProviderSettings,
    session_id: &str,
    turn_id: &str,
    api_key: Option<&str>,
) -> Result<(), ProviderFailureKind> {
    if !safe_remote_id(turn_id) {
        return Err(ProviderFailureKind::Protocol);
    }
    let mut url = session_operation_url(provider, session_id, "turns")?;
    url.path_segments_mut()
        .map_err(|_| ProviderFailureKind::Contract)?
        .push(turn_id)
        .push("cancel");
    send(authorized(client.post(url), api_key).header("Idempotency-Key", idempotency_key()))
        .await
        .map(|_| ())
}

async fn read_connection(
    response: Response,
    session: &SessionResponse,
    turn_id: &str,
    deadline: TokioInstant,
    context: &ModelStreamContext<'_>,
    state: &mut StreamState,
) -> ReadResult {
    let mut chunks = response.bytes_stream();
    let mut buffer = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = context.cancellation.cancelled() => return ReadResult::Cancelled,
            _ = tokio::time::sleep_until(deadline) => return ReadResult::Failed(ProviderFailureKind::Timeout),
            chunk = chunks.next() => chunk,
        };
        let Some(chunk) = chunk else {
            return ReadResult::Reconnect;
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => return ReadResult::Reconnect,
        };
        if buffer.len().saturating_add(chunk.len()) > MAX_SSE_EVENT_BYTES {
            return ReadResult::Failed(ProviderFailureKind::RequestTooLarge);
        }
        buffer.extend_from_slice(&chunk);
        while let Some(block) = take_event(&mut buffer) {
            let parsed = match parse_event(&block) {
                Ok(Some(parsed)) => parsed,
                Ok(None) => continue,
                Err(kind) => return ReadResult::Failed(kind),
            };
            if let Some(outcome) = accept_event(parsed, session, turn_id, context, state) {
                return outcome;
            }
        }
    }
}

fn accept_event(
    parsed: ParsedEvent,
    session: &SessionResponse,
    turn_id: &str,
    context: &ModelStreamContext<'_>,
    state: &mut StreamState,
) -> Option<ReadResult> {
    let event = parsed.payload;
    if parsed
        .event_name
        .as_deref()
        .is_some_and(|name| name != event.event_type)
        || event.session_id != session.id
        || event
            .cursor
            .as_deref()
            .zip(parsed.id.as_deref())
            .is_some_and(|(cursor, id)| cursor != id)
    {
        return Some(ReadResult::Failed(ProviderFailureKind::Protocol));
    }
    if let Some(cursor) = parsed.id.or(event.cursor) {
        if cursor.is_empty() || cursor.len() > 4_096 || cursor.chars().any(char::is_control) {
            return Some(ReadResult::Failed(ProviderFailureKind::Protocol));
        }
        state.last_cursor = Some(cursor);
    }
    let event_turn_id = event.turn_id.as_deref()?;
    if event_turn_id != turn_id {
        return Some(ReadResult::Failed(ProviderFailureKind::Protocol));
    }
    match event.event_type.as_str() {
        "message.delta" => append_text(event.data.get("text"), context, state),
        "message.completed" if state.content.is_empty() => {
            append_text(event.data.get("text"), context, state)
        }
        "turn.completed" if state.content.is_empty() => Some(ReadResult::Terminal(failed(
            ProviderFailureKind::Protocol,
            false,
        ))),
        "turn.completed" => Some(ReadResult::Terminal(ProviderAttemptOutcome::Completed {
            content: std::mem::take(&mut state.content),
            cleanup: CleanupOutcome::NotStarted,
        })),
        "turn.cancelled" => Some(ReadResult::Terminal(super::cancelled(state.output_started))),
        "turn.failed" => Some(ReadResult::Terminal(failed(
            ProviderFailureKind::Upstream,
            state.output_started,
        ))),
        "turn.unqueued" => Some(ReadResult::Terminal(failed(
            ProviderFailureKind::Capacity,
            state.output_started,
        ))),
        "approval.requested" | "user_input.requested" => {
            Some(ReadResult::Failed(ProviderFailureKind::Policy))
        }
        _ => None,
    }
}

fn append_text(
    value: Option<&Value>,
    context: &ModelStreamContext<'_>,
    state: &mut StreamState,
) -> Option<ReadResult> {
    let Some(text) = value.and_then(Value::as_str) else {
        return Some(ReadResult::Failed(ProviderFailureKind::Protocol));
    };
    if text.is_empty() {
        return None;
    }
    let chars = text.chars().count();
    if state.content.len().saturating_add(text.len()) > MAX_CONTENT_BYTES
        || state.content_chars.saturating_add(chars) > MAX_CONTENT_CHARS
    {
        return Some(ReadResult::Failed(ProviderFailureKind::RequestTooLarge));
    }
    if !state.output_started {
        if context
            .output_persistence
            .is_some_and(|persistence| persistence.mark_started().is_err())
        {
            return Some(ReadResult::Failed(ProviderFailureKind::Internal));
        }
        state.output_started = true;
    }
    state.content.push_str(text);
    state.content_chars += chars;
    if context
        .on_event
        .send_received(
            RuntimeEvent::Delta {
                run_id: context.input.run_id.clone(),
                text: text.to_string(),
            },
            Instant::now(),
        )
        .is_err()
    {
        return Some(ReadResult::Failed(ProviderFailureKind::ClientDisconnected));
    }
    None
}

fn is_event_stream(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
}

fn take_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let (start, delimiter) = (0..buffer.len()).find_map(|index| {
        if buffer.get(index..index + 2) == Some(b"\n\n") {
            Some((index, 2))
        } else if buffer.get(index..index + 4) == Some(b"\r\n\r\n") {
            Some((index, 4))
        } else {
            None
        }
    })?;
    let mut consumed = buffer.drain(..start + delimiter).collect::<Vec<_>>();
    consumed.truncate(start);
    Some(consumed)
}

fn parse_event(block: &[u8]) -> Result<Option<ParsedEvent>, ProviderFailureKind> {
    let text = std::str::from_utf8(block).map_err(|_| ProviderFailureKind::Protocol)?;
    let mut event_name = None;
    let mut id = None;
    let mut data = Vec::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => event_name = Some(value.to_string()),
            "id" => id = Some(value.to_string()),
            "data" => data.push(value),
            _ => {}
        }
    }
    if data.is_empty() {
        return Ok(None);
    }
    let payload =
        serde_json::from_str(&data.join("\n")).map_err(|_| ProviderFailureKind::Protocol)?;
    Ok(Some(ParsedEvent {
        event_name,
        id,
        payload,
    }))
}

fn idempotency_key() -> String {
    format!("saaa_{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_crlf_events_and_ignores_heartbeats() {
        let mut buffer = b": heartbeat\r\n\r\nid: c1\r\nevent: message.delta\r\ndata: {\"type\":\"message.delta\",\"session_id\":\"ags_1\",\"turn_id\":\"agt_1\",\"cursor\":\"c1\",\"data\":{\"text\":\"ok\"}}\r\n\r\n".to_vec();
        assert!(parse_event(&take_event(&mut buffer).expect("heartbeat"))
            .unwrap()
            .is_none());
        let parsed = parse_event(&take_event(&mut buffer).expect("event"))
            .unwrap()
            .expect("payload");
        assert_eq!(parsed.event_name.as_deref(), Some("message.delta"));
        assert_eq!(parsed.payload.data["text"], "ok");
        assert!(buffer.is_empty());
    }

    #[test]
    fn serializes_role_preserving_conversation_input() {
        let message = |role: &str, content: &str| ConversationMessage {
            parts: None,
            id: "id".to_string(),
            conversation_id: "conversation".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at: "now".to_string(),
        };
        let input = render_turn_input(&[
            message("system", "policy"),
            message("assistant", "prior"),
            message("user", "question"),
        ])
        .unwrap();
        let value: Value = serde_json::from_str(&input).unwrap();
        assert_eq!(value["messages"][0]["role"], "system");
        assert_eq!(value["messages"][2]["content"], "question");
    }

    #[test]
    fn folds_delta_and_terminal_events_into_a_completed_attempt() {
        let input = crate::StartTurnInput {
            run_id: "run".to_string(),
            conversation_id: "conversation".to_string(),
            content: "question".to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        let sink = tauri::ipc::Channel::new(|_| Ok(()));
        let cancellation = std::sync::Arc::new(crate::RunCancellation::default());
        let context = ModelStreamContext {
            reasoning_effort: "medium",
            max_output_tokens: 64,
            input: &input,
            on_event: &sink,
            cancellation,
            output_persistence: None,
        };
        let session = SessionResponse {
            id: "ags_1".to_string(),
            stream_url: None,
            stream_protocol: None,
            events_url: Some("/events".to_string()),
        };
        let event = |event_type: &str, cursor: &str, data: Value| ParsedEvent {
            event_name: Some(event_type.to_string()),
            id: Some(cursor.to_string()),
            payload: AgentEvent {
                event_type: event_type.to_string(),
                session_id: "ags_1".to_string(),
                turn_id: Some("agt_1".to_string()),
                cursor: Some(cursor.to_string()),
                data,
            },
        };
        let mut state = StreamState::default();
        assert!(accept_event(
            event("message.delta", "c1", json!({ "text": "ready" })),
            &session,
            "agt_1",
            &context,
            &mut state,
        )
        .is_none());
        let terminal = accept_event(
            event("turn.completed", "c2", json!({})),
            &session,
            "agt_1",
            &context,
            &mut state,
        );
        assert!(matches!(
            terminal,
            Some(ReadResult::Terminal(ProviderAttemptOutcome::Completed { content, .. }))
                if content == "ready"
        ));
    }
}
