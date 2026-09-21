//! Bounded SSE reading and protocol validation.
use super::*;
#[allow(clippy::too_many_arguments)]
pub(super) async fn read_turn(
    client: &Client,
    provider: &AgentSessionProviderSettings,
    session: &SessionResponse,
    events_url: &Url,
    turn: &TurnResponse,
    deadline: TokioInstant,
    api_key: Option<&str>,
    context: &ModelStreamContext<'_>,
    state: &mut StreamState,
) -> ProviderAttemptOutcome {
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
        match read_connection(response, session, &turn.id, deadline, context, state).await {
            ReadResult::Reconnect if reconnects < MAX_RECONNECTS && state.last_cursor.is_some() => {
                reconnects += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            result => break result,
        }
    };
    let (attempt, terminal) = match result {
        ReadResult::Terminal(outcome) => (outcome, true),
        ReadResult::Cancelled => (super::super::cancelled(state.output_started), false),
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

pub(super) fn accept_event(
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
        if state.last_cursor.as_deref() == Some(cursor.as_str()) {
            return None;
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
            state.output_started,
        ))),
        "turn.completed" => Some(ReadResult::Terminal(ProviderAttemptOutcome::Completed {
            content: std::mem::take(&mut state.content),
            cleanup: CleanupOutcome::NotStarted,
        })),
        "turn.cancelled" => Some(ReadResult::Terminal(super::super::cancelled(
            state.output_started,
        ))),
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
    state.content.push_str(text);
    state.content_chars += chars;
    let visible = match state.projection.push(text) {
        Ok(text) => text,
        Err(()) => return Some(ReadResult::Failed(ProviderFailureKind::RequestTooLarge)),
    };
    if visible.is_empty() {
        return None;
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
    if context
        .on_event
        .send_received(
            RuntimeEvent::Delta {
                run_id: context.input.run_id.clone(),
                text: visible,
            },
            Instant::now(),
        )
        .is_err()
    {
        return Some(ReadResult::Failed(ProviderFailureKind::ClientDisconnected));
    }
    None
}

pub(super) fn is_event_stream(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
}

pub(super) fn take_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
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

pub(super) fn parse_event(block: &[u8]) -> Result<Option<ParsedEvent>, ProviderFailureKind> {
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

pub(super) fn idempotency_key() -> String {
    format!("saaa_{}", uuid::Uuid::new_v4().simple())
}
