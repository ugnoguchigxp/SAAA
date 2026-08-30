use futures_util::StreamExt;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use url::Url;

use super::super::contracts::{
    AllocationBindingDto, AllocationDto, AllocationStatus, BoundedIdentifier, OperationDto,
    PendingAllocation, ReadyAllocation, ReleaseFailureKind, SelectionReason, SessionFailureKind,
};
use super::{
    Cancellation, ErrorEnvelope, LarmError, LarmHttpClient, ASSISTANT_CHAR_LIMIT, CAPABILITY,
    ERROR_BODY_LIMIT, PROBE_BODY_LIMIT, ROUTE, SSE_EVENT_LIMIT,
};
use crate::providers::completion::{CompletionTerminal, CompletionTerminalError};
use crate::providers::openai_compatible::sse_event_data;
use crate::runtime::agent_tools::{ToolCallAccumulator, ToolProtocolError};

pub(crate) fn validate_allocation_common(dto: &AllocationDto) -> Result<(), LarmError> {
    BoundedIdentifier::new(dto.id.clone())
        .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))?;
    dto.operation_id
        .as_ref()
        .map(|value| BoundedIdentifier::new(value.clone()))
        .transpose()
        .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))?;
    if dto.requirements.len() != 1
        || dto.bindings.len() != 1
        || dto.requirements[0].capability != CAPABILITY
        || dto.requirements[0].route != ROUTE
        || dto.allow_fallback
        || dto.deployment_policy != "existing-only"
        || dto.created_at.is_empty()
        || dto.created_at.len() > 64
        || !valid_datetime(&dto.created_at)
        || dto.expires_at.is_empty()
        || dto.expires_at.len() > 64
        || !valid_datetime(&dto.expires_at)
        || dto
            .client
            .as_ref()
            .is_some_and(|value| value != "saaa-desktop")
        || dto
            .released_at
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 64 || !valid_datetime(value))
        || dto.error.as_ref().is_some_and(|error| {
            !valid_external_text(&error.code, 128) || !valid_external_text(&error.message, 1_024)
        })
    {
        return Err(LarmError::new(SessionFailureKind::Protocol, false));
    }
    let binding = &dto.bindings[0];
    if binding.capability != CAPABILITY
        || binding.route != ROUTE
        || !valid_external_text(&binding.status, 128)
        || !valid_external_text(&binding.selection_reason, 128)
        || binding.candidate_rank == 0
        || binding.fallback
    {
        return Err(LarmError::new(SessionFailureKind::Protocol, false));
    }
    BoundedIdentifier::new(binding.runtime.clone())
        .and_then(|_| BoundedIdentifier::new(binding.node.clone()))
        .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))?;
    Ok(())
}

pub(crate) fn valid_external_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

pub(crate) fn valid_datetime(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !value.is_ascii() || !(20..=30).contains(&bytes.len()) {
        return false;
    }
    if bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
        || bytes.last() != Some(&b'Z')
    {
        return false;
    }
    for range in [0..4, 5..7, 8..10, 11..13, 14..16, 17..19] {
        if !bytes[range].iter().all(u8::is_ascii_digit) {
            return false;
        }
    }
    let parse = |range: std::ops::Range<usize>| {
        std::str::from_utf8(&bytes[range])
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
    };
    let Some(year) = parse(0..4) else {
        return false;
    };
    let Some(month @ 1..=12) = parse(5..7) else {
        return false;
    };
    let Some(day) = parse(8..10) else {
        return false;
    };
    let leap_year = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        2 if leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=max_day).contains(&day)
        || !matches!(parse(11..13), Some(0..=23))
        || !matches!(parse(14..16), Some(0..=59))
        || !matches!(parse(17..19), Some(0..=59))
    {
        return false;
    }
    bytes.len() == 20
        || (bytes.len() >= 22
            && bytes.get(19) == Some(&b'.')
            && bytes[20..bytes.len() - 1].iter().all(u8::is_ascii_digit))
}

pub(crate) fn operation_failure_kind(operation: &OperationDto) -> SessionFailureKind {
    operation
        .error
        .as_ref()
        .map(|error| classify_operation_error_code(&error.code))
        .unwrap_or(SessionFailureKind::Unavailable)
}

pub(crate) fn classify_operation_error_code(code: &str) -> SessionFailureKind {
    match code {
        "bad_request"
        | "allocation_required"
        | "duplicate_capability"
        | "unknown_route"
        | "unsupported_capability"
        | "capability_not_allocated" => SessionFailureKind::Contract,
        "request_cancelled" => SessionFailureKind::Cancelled,
        "unauthorized" | "forbidden" => SessionFailureKind::Authentication,
        "not_found" | "allocation_inactive" => SessionFailureKind::AllocationLost,
        "fallback_not_allowed" => SessionFailureKind::Policy,
        "resource_exhausted" | "runtime_in_use" | "deployment_in_progress" => {
            SessionFailureKind::Capacity
        }
        "body_too_large" => SessionFailureKind::RequestTooLarge,
        "upstream_unavailable" | "upstream_error" | "internal_error" => {
            SessionFailureKind::Upstream
        }
        "no_candidate_available"
        | "stale_state"
        | "management_not_configured"
        | "runtime_not_ready" => SessionFailureKind::Unavailable,
        "allocation_not_ready" => SessionFailureKind::NotReady,
        "draining" => SessionFailureKind::Draining,
        "gateway_timeout" => SessionFailureKind::Timeout,
        _ => SessionFailureKind::Protocol,
    }
}

pub(crate) fn binding_fingerprint(binding: &AllocationBindingDto) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}",
        binding.capability,
        binding.route,
        binding.runtime,
        binding.node,
        binding.status,
        binding.candidate_rank,
        binding.fallback
    )
    .bytes()
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("binding_{hash:016x}")
}

pub(crate) fn binding_identity(binding: &AllocationBindingDto) -> String {
    let fields = [
        binding.capability.as_str(),
        binding.route.as_str(),
        binding.runtime.as_str(),
        binding.node.as_str(),
        binding.status.as_str(),
    ];
    let mut identity = fields
        .iter()
        .map(|field| format!("{}:{field}", field.len()))
        .collect::<Vec<_>>()
        .join("|");
    identity.push_str(&format!(
        "|{}|{}",
        binding.candidate_rank,
        u8::from(binding.fallback)
    ));
    identity
}

pub(crate) fn parse_base_url(value: &str) -> Result<Url, SessionFailureKind> {
    let url = Url::parse(value).map_err(|_| SessionFailureKind::Contract)?;
    let numeric_loopback = matches!(
        url.host(),
        Some(url::Host::Ipv4(address)) if address == std::net::Ipv4Addr::LOCALHOST
    ) || matches!(
        url.host(),
        Some(url::Host::Ipv6(address)) if address == std::net::Ipv6Addr::LOCALHOST
    );
    if url.scheme() != "http"
        || !numeric_loopback
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(SessionFailureKind::Contract);
    }
    Ok(url)
}

pub(crate) fn media_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

pub(crate) async fn decode_json_limited<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    limit: usize,
    cancellation: Cancellation<'_>,
    output_started: bool,
) -> Result<T, LarmError> {
    let body = read_body_limited(response, limit, Some(cancellation), output_started).await?;
    serde_json::from_slice(&body)
        .map_err(|_| LarmError::new(SessionFailureKind::Protocol, output_started))
}

pub(crate) async fn read_body_limited(
    response: reqwest::Response,
    limit: usize,
    cancellation: Option<Cancellation<'_>>,
    output_started: bool,
) -> Result<Vec<u8>, LarmError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(LarmError::new(
            SessionFailureKind::RequestTooLarge,
            output_started,
        ));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let next = if let Some(cancellation) = cancellation {
            tokio::select! {
                _ = cancellation.cancelled() => return Err(LarmError::new(SessionFailureKind::Cancelled, output_started)),
                next = stream.next() => next,
            }
        } else {
            stream.next().await
        };
        let Some(chunk) = next else {
            return Ok(body);
        };
        let chunk =
            chunk.map_err(|error| LarmError::new(classify_transport(&error), output_started))?;
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(LarmError::new(
                SessionFailureKind::RequestTooLarge,
                output_started,
            ));
        }
        body.extend_from_slice(&chunk);
    }
}

pub(crate) async fn classify_error_response(
    response: reqwest::Response,
    limit: usize,
) -> SessionFailureKind {
    let status = response.status();
    let body = match read_body_limited(response, limit.min(ERROR_BODY_LIMIT), None, false).await {
        Ok(body) => body,
        Err(error) => return error.kind,
    };
    let envelope: ErrorEnvelope = match serde_json::from_slice(&body) {
        Ok(envelope) => envelope,
        Err(_) => return SessionFailureKind::Protocol,
    };
    if !valid_external_text(&envelope.error.code, 128)
        || !valid_external_text(&envelope.error.message, 1_024)
    {
        return SessionFailureKind::Protocol;
    }
    classify_error_code(&envelope.error.code, status)
}

pub(crate) async fn classify_probe_response(response: reqwest::Response) -> SessionFailureKind {
    let status = response.status();
    let body = match read_body_limited(response, PROBE_BODY_LIMIT, None, false).await {
        Ok(body) => body,
        Err(error) => return error.kind,
    };
    if let Ok(value) = serde_json::from_slice::<Value>(&body) {
        if matches!(
            value.get("status").and_then(Value::as_str),
            Some("stale" | "draining")
        ) {
            return SessionFailureKind::Unavailable;
        }
    }
    let envelope: ErrorEnvelope = match serde_json::from_slice(&body) {
        Ok(envelope) => envelope,
        Err(_) => return SessionFailureKind::Protocol,
    };
    if !valid_external_text(&envelope.error.code, 128)
        || !valid_external_text(&envelope.error.message, 1_024)
    {
        return SessionFailureKind::Protocol;
    }
    classify_error_code(&envelope.error.code, status)
}

pub(crate) fn classify_error_code(code: &str, status: StatusCode) -> SessionFailureKind {
    match (status.as_u16(), code) {
        (400, "bad_request" | "allocation_required" | "duplicate_capability") => {
            SessionFailureKind::Contract
        }
        (400, "request_cancelled") => SessionFailureKind::Cancelled,
        (401, "unauthorized") | (403, "forbidden") => SessionFailureKind::Authentication,
        (404, "unknown_route" | "unsupported_capability" | "capability_not_allocated") => {
            SessionFailureKind::Contract
        }
        (404, "not_found") => SessionFailureKind::AllocationLost,
        (409, "fallback_not_allowed") => SessionFailureKind::Policy,
        (409, "resource_exhausted" | "runtime_in_use") => SessionFailureKind::Capacity,
        (409, "deployment_in_progress") => SessionFailureKind::Capacity,
        (409, "allocation_inactive") => SessionFailureKind::AllocationLost,
        (413, "body_too_large") => SessionFailureKind::RequestTooLarge,
        (502, "upstream_unavailable" | "upstream_error") => SessionFailureKind::Upstream,
        (
            503,
            "no_candidate_available"
            | "stale_state"
            | "management_not_configured"
            | "runtime_not_ready",
        ) => SessionFailureKind::Unavailable,
        (503, "allocation_not_ready" | "deployment_in_progress") => SessionFailureKind::NotReady,
        (503, "draining") => SessionFailureKind::Draining,
        (504, "gateway_timeout") => SessionFailureKind::Timeout,
        (500..=599, "internal_error") => SessionFailureKind::Upstream,
        _ => SessionFailureKind::Protocol,
    }
}

pub(crate) fn classify_transport(error: &reqwest::Error) -> SessionFailureKind {
    if error.is_timeout() {
        SessionFailureKind::Timeout
    } else if error.is_builder() {
        SessionFailureKind::Internal
    } else {
        SessionFailureKind::Network
    }
}

pub(crate) fn should_retry_stream_renew(kind: SessionFailureKind, previous_retries: u8) -> bool {
    kind.permits_stream_renew_retry() && previous_retries == 0
}

pub(crate) fn lease_deadlines(
    received_at: tokio::time::Instant,
    effective_ttl_seconds: u32,
) -> (Option<tokio::time::Instant>, tokio::time::Instant) {
    let ttl_seconds = u64::from(effective_ttl_seconds);
    let ttl = Duration::from_secs(ttl_seconds);
    let renew_after = Duration::from_secs(ttl_seconds * 4 / 5);
    (Some(received_at + renew_after), received_at + ttl)
}

pub(crate) fn release_kind(kind: SessionFailureKind) -> ReleaseFailureKind {
    match kind {
        SessionFailureKind::Authentication => ReleaseFailureKind::Authentication,
        SessionFailureKind::Protocol | SessionFailureKind::AllocationLost => {
            ReleaseFailureKind::Protocol
        }
        SessionFailureKind::Upstream | SessionFailureKind::Unavailable => {
            ReleaseFailureKind::Upstream
        }
        SessionFailureKind::Network => ReleaseFailureKind::Network,
        SessionFailureKind::Timeout => ReleaseFailureKind::Timeout,
        _ => ReleaseFailureKind::Internal,
    }
}

pub(crate) fn drain_sse(
    buffer: &mut Vec<u8>,
    output_started: bool,
) -> Result<Vec<String>, LarmError> {
    let mut events = Vec::new();
    loop {
        let lf = buffer.windows(2).position(|window| window == b"\n\n");
        let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
        let boundary = match (lf, crlf) {
            (Some(lf), Some(crlf)) if lf < crlf => Some((lf, 2)),
            (Some(_), Some(crlf)) => Some((crlf, 4)),
            (Some(lf), None) => Some((lf, 2)),
            (None, Some(crlf)) => Some((crlf, 4)),
            (None, None) => None,
        };
        let Some((index, delimiter_length)) = boundary else {
            break;
        };
        if index > SSE_EVENT_LIMIT {
            return Err(LarmError::new(
                SessionFailureKind::RequestTooLarge,
                output_started,
            ));
        }
        let drained = buffer.drain(..index + delimiter_length).collect::<Vec<_>>();
        let event = String::from_utf8(drained[..index].to_vec())
            .map_err(|_| LarmError::new(SessionFailureKind::Protocol, output_started))?;
        events.push(event);
    }
    Ok(events)
}

pub(crate) fn project_sse<F>(
    events: Vec<String>,
    content: &mut String,
    content_chars: &mut usize,
    output_started: &mut bool,
    on_delta: &mut F,
    tool_calls: &mut ToolCallAccumulator,
    terminal: &mut CompletionTerminal,
) -> Result<bool, LarmError>
where
    F: FnMut(&str, bool) -> Result<(), SessionFailureKind>,
{
    for event in events {
        let Some(data) = sse_event_data(&event) else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            terminal
                .complete()
                .map_err(|error| completion_terminal_error(error, *output_started))?;
            return Ok(true);
        }
        let value: Value = serde_json::from_str(data)
            .map_err(|_| LarmError::new(SessionFailureKind::Protocol, *output_started))?;
        terminal
            .observe(&value)
            .map_err(|error| completion_terminal_error(error, *output_started))?;
        tool_calls
            .absorb_stream_delta(&value)
            .map_err(|error| tool_protocol_error(error, *output_started))?;
        if let Some(delta) = value
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            let delta_chars = delta.chars().count();
            if delta_chars == 0 {
                continue;
            }
            if *content_chars + delta_chars > ASSISTANT_CHAR_LIMIT {
                return Err(LarmError::new(
                    SessionFailureKind::RequestTooLarge,
                    *output_started,
                ));
            }
            let first = !*output_started;
            on_delta(delta, first).map_err(|kind| LarmError::new(kind, true))?;
            content.push_str(delta);
            *content_chars += delta_chars;
            *output_started = true;
        }
    }
    Ok(false)
}

pub(crate) fn completion_terminal_error(
    error: CompletionTerminalError,
    output_started: bool,
) -> LarmError {
    let kind = match error {
        CompletionTerminalError::PartialOutput => SessionFailureKind::PartialOutput,
        CompletionTerminalError::Policy => SessionFailureKind::Policy,
        CompletionTerminalError::Protocol => SessionFailureKind::Protocol,
    };
    LarmError::new(kind, output_started)
}

pub(crate) fn tool_protocol_error(error: ToolProtocolError, output_started: bool) -> LarmError {
    let kind = match error {
        ToolProtocolError::Protocol => SessionFailureKind::Protocol,
        ToolProtocolError::TooLarge => SessionFailureKind::RequestTooLarge,
    };
    LarmError::new(kind, output_started)
}

impl LarmHttpClient<'_> {
    pub(super) fn ready_allocation(
        &self,
        dto: AllocationDto,
    ) -> Result<ReadyAllocation, LarmError> {
        validate_allocation_common(&dto)?;
        if dto.status != AllocationStatus::Ready || dto.bindings.len() != 1 {
            return Err(LarmError::new(SessionFailureKind::Protocol, false));
        }
        let binding = &dto.bindings[0];
        if binding.capability != CAPABILITY
            || binding.route != ROUTE
            || binding.fallback
            || binding.candidate_rank == 0
            || !valid_external_text(&binding.status, 128)
        {
            return Err(LarmError::new(SessionFailureKind::Protocol, false));
        }
        BoundedIdentifier::new(binding.node.clone())
            .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))?;
        let runtime = BoundedIdentifier::new(binding.runtime.clone())
            .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))?;
        let allocation_id = BoundedIdentifier::new(dto.id)
            .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))?;
        let fingerprint = binding_fingerprint(binding);
        ReadyAllocation::new_with_binding_identity(
            allocation_id.as_str(),
            runtime.as_str(),
            fingerprint,
            binding_identity(binding),
            self.ttl_seconds,
            binding.fallback,
            if matches!(
                binding.selection_reason.as_str(),
                "primary-live" | "primary-startable"
            ) {
                SelectionReason::Primary
            } else {
                SelectionReason::Other
            },
        )
        .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))
    }

    pub(super) fn pending_allocation(
        &self,
        dto: AllocationDto,
    ) -> Result<PendingAllocation, LarmError> {
        validate_allocation_common(&dto)?;
        if dto.status != AllocationStatus::Pending {
            return Err(LarmError::new(SessionFailureKind::Protocol, false));
        }
        let operation_id = dto
            .operation_id
            .ok_or_else(|| LarmError::new(SessionFailureKind::Protocol, false))?;
        PendingAllocation::new(operation_id, Some(dto.id))
            .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))
    }

    pub(super) fn validate_operation(
        &self,
        operation: &OperationDto,
        expected_operation_id: &BoundedIdentifier,
        expected_allocation_id: Option<&BoundedIdentifier>,
    ) -> Result<(), LarmError> {
        let id = BoundedIdentifier::new(operation.id.clone())
            .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))?;
        if &id != expected_operation_id || operation.kind != "allocation" {
            return Err(LarmError::new(SessionFailureKind::Protocol, false));
        }
        let allocation_id = operation
            .allocation_id
            .as_ref()
            .ok_or_else(|| LarmError::new(SessionFailureKind::Protocol, false))?;
        let actual = BoundedIdentifier::new(allocation_id.clone())
            .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))?;
        if expected_allocation_id.is_some_and(|expected| expected != &actual) {
            return Err(LarmError::new(SessionFailureKind::Protocol, false));
        }
        if operation.created_at.is_empty()
            || operation.created_at.len() > 64
            || !valid_datetime(&operation.created_at)
            || operation.desired.len() > 16
            || operation.ensure.len() > 16
            || operation
                .phase
                .as_ref()
                .is_some_and(|value| !valid_external_text(value, 128))
            || operation
                .deadline_at
                .as_ref()
                .is_some_and(|value| value.len() > 64 || !valid_datetime(value))
            || operation
                .completed_at
                .as_ref()
                .is_some_and(|value| value.len() > 64 || !valid_datetime(value))
        {
            return Err(LarmError::new(SessionFailureKind::Protocol, false));
        }
        for identifier in operation.desired.iter().chain(&operation.ensure) {
            BoundedIdentifier::new(identifier.clone())
                .map_err(|_| LarmError::new(SessionFailureKind::Protocol, false))?;
        }
        if operation.error.as_ref().is_some_and(|error| {
            !valid_external_text(&error.code, 128) || !valid_external_text(&error.message, 1_024)
        }) {
            return Err(LarmError::new(SessionFailureKind::Protocol, false));
        }
        Ok(())
    }
}
