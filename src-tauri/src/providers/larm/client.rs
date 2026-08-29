use super::contracts::{
    AllocationDto, AllocationStatus, BoundedIdentifier, OperationDto, OperationStatus,
    PendingAllocation, ReadyAllocation, ReleaseFailureKind, SelectionReason, SessionFailureKind,
};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Method, StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    future::pending,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::Notify;
use url::Url;

use crate::runtime::agent_tools::{AgentToolCall, ToolCallAccumulator, ToolProtocolError};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);
const PROBE_BODY_LIMIT: usize = 64 * 1_024;
const CONTROL_BODY_LIMIT: usize = 256 * 1_024;
const RELEASE_BODY_LIMIT: usize = 64 * 1_024;
const ERROR_BODY_LIMIT: usize = 8 * 1_024;
const GATEWAY_REQUEST_LIMIT: usize = 4 * 1_024 * 1_024;
const SSE_EVENT_LIMIT: usize = 1_024 * 1_024;
const ASSISTANT_CHAR_LIMIT: usize = 64_000;
const CAPABILITY: &str = "llm.general";
const ROUTE: &str = "llm-default";
const VIRTUAL_MODEL: &str = "local";

#[derive(Clone)]
pub(crate) struct SharedLarmClient(reqwest::Client);

impl SharedLarmClient {
    pub(crate) fn build() -> Result<Self, SessionFailureKind> {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map(Self)
            .map_err(|_| SessionFailureKind::Internal)
    }

    #[cfg(test)]
    pub(crate) async fn canary_authentication_boundary(
        &self,
        base_url: &str,
    ) -> Result<(), SessionFailureKind> {
        let base_url = parse_base_url(base_url)?;
        let synthetic_id = format!("canary_{}", uuid::Uuid::new_v4().simple());
        let url = base_url
            .join(&format!("v1/allocations/{synthetic_id}"))
            .map_err(|_| SessionFailureKind::Contract)?;
        let correct = EphemeralCredential::from_environment()?;
        let incorrect = EphemeralCredential::from_token(&format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ))?;
        for (credential, expected) in [
            (None, SessionFailureKind::Authentication),
            (Some(&incorrect), SessionFailureKind::Authentication),
            (Some(&correct), SessionFailureKind::AllocationLost),
        ] {
            let mut request = self.0.get(url.clone()).timeout(PROBE_TIMEOUT);
            if let Some(credential) = credential {
                request = request.header(AUTHORIZATION, credential.0.clone());
            }
            let response = request
                .send()
                .await
                .map_err(|error| classify_transport(&error))?;
            if classify_error_response(response, ERROR_BODY_LIMIT).await != expected {
                return Err(SessionFailureKind::Contract);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn canary_health(&self, base_url: &str) -> Result<(), SessionFailureKind> {
        LarmHttpClient::new_probe(self, base_url)?
            .probe_endpoint("health", "ok")
            .await
    }

    #[cfg(test)]
    pub(crate) async fn canary_ready(&self, base_url: &str) -> Result<(), SessionFailureKind> {
        LarmHttpClient::new_probe(self, base_url)?
            .probe_endpoint("ready", "ready")
            .await
    }

    #[cfg(test)]
    pub(crate) async fn canary_active_allocations(
        &self,
        base_url: &str,
    ) -> Result<u64, SessionFailureKind> {
        const METRICS_LIMIT: usize = 1_024 * 1_024;
        const METRICS_LINE_LIMIT: usize = 16 * 1_024;
        const ACTIVE_METRIC: &str = "larm_active_allocations";
        let metrics_scope = std::env::var("SAAA_LARM_CANARY_METRICS_SCOPE")
            .map_err(|_| SessionFailureKind::Contract)?;
        if !matches!(metrics_scope.as_str(), "exclusive-window" | "client-scoped") {
            return Err(SessionFailureKind::Contract);
        }
        let base_url = parse_base_url(base_url)?;
        let token =
            std::env::var("LARM_API_TOKEN").map_err(|_| SessionFailureKind::Authentication)?;
        let credential = EphemeralCredential::from_token(&token)?;
        let response = self
            .0
            .get(
                base_url
                    .join("metrics")
                    .map_err(|_| SessionFailureKind::Contract)?,
            )
            .header(AUTHORIZATION, credential.0)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .map_err(|error| classify_transport(&error))?;
        let status = response.status();
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(SessionFailureKind::Authentication);
        }
        if status.is_server_error() {
            return Err(SessionFailureKind::Upstream);
        }
        if !status.is_success() {
            return Err(SessionFailureKind::Contract);
        }
        if response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_none_or(|value| media_type(value) != "text/plain")
        {
            return Err(SessionFailureKind::Contract);
        }
        let body = read_body_limited(response, METRICS_LIMIT, None, false)
            .await
            .map_err(|error| error.kind)?;
        let text = std::str::from_utf8(&body).map_err(|_| SessionFailureKind::Protocol)?;
        let endpoint = base_url.as_str();
        let endpoint_without_trailing_slash = endpoint.strip_suffix('/').unwrap_or(endpoint);
        if text.contains(&token)
            || text.contains(endpoint)
            || text.contains(endpoint_without_trailing_slash)
            || super::CANARY_PROMPTS
                .iter()
                .any(|value| text.contains(value))
        {
            return Err(SessionFailureKind::Policy);
        }
        let mut active = None;
        for line in text.lines() {
            if line.len() > METRICS_LINE_LIMIT {
                return Err(SessionFailureKind::RequestTooLarge);
            }
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split_ascii_whitespace();
            let Some(series) = fields.next() else {
                continue;
            };
            let Some(value) = fields.next() else {
                return Err(SessionFailureKind::Protocol);
            };
            if fields.next().is_some() {
                return Err(SessionFailureKind::Protocol);
            }
            if metric_series_has_sensitive_label(series)? {
                return Err(SessionFailureKind::Policy);
            }
            let allowed_series = match metrics_scope.as_str() {
                "exclusive-window" => series == ACTIVE_METRIC,
                "client-scoped" => series == format!(r#"{ACTIVE_METRIC}{{client="saaa-desktop"}}"#),
                _ => false,
            };
            if !allowed_series {
                continue;
            }
            if active.is_some() {
                return Err(SessionFailureKind::Protocol);
            }
            active = Some(
                value
                    .parse::<u64>()
                    .map_err(|_| SessionFailureKind::Protocol)?,
            );
        }
        active.ok_or(SessionFailureKind::Contract)
    }
}

#[cfg(test)]
fn metric_series_has_sensitive_label(series: &str) -> Result<bool, SessionFailureKind> {
    let Some(open) = series.find('{') else {
        return if series.contains('}') {
            Err(SessionFailureKind::Protocol)
        } else {
            Ok(false)
        };
    };
    if !series.ends_with('}') || series[..open].contains('}') {
        return Err(SessionFailureKind::Protocol);
    }
    let labels = &series[open + 1..series.len() - 1];
    let mut segment_start = 0;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in labels.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quoted {
            escaped = true;
        } else if character == '"' {
            quoted = !quoted;
        } else if character == ',' && !quoted {
            if sensitive_metric_label_segment(&labels[segment_start..index])? {
                return Ok(true);
            }
            segment_start = index + character.len_utf8();
        }
    }
    if quoted || escaped {
        return Err(SessionFailureKind::Protocol);
    }
    sensitive_metric_label_segment(&labels[segment_start..])
}

#[cfg(test)]
fn sensitive_metric_label_segment(segment: &str) -> Result<bool, SessionFailureKind> {
    let segment = segment.trim();
    if segment.is_empty() {
        return Ok(false);
    }
    let (key, _) = segment
        .split_once('=')
        .ok_or(SessionFailureKind::Protocol)?;
    let key = key.trim();
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(SessionFailureKind::Protocol);
    }
    let compact = key
        .bytes()
        .filter(|byte| *byte != b'_')
        .map(|byte| byte.to_ascii_lowercase())
        .collect::<Vec<_>>();
    Ok(matches!(
        compact.as_slice(),
        b"authorization"
            | b"bearer"
            | b"token"
            | b"prompt"
            | b"response"
            | b"endpoint"
            | b"identifier"
            | b"allocationid"
            | b"operationid"
            | b"requestid"
            | b"conversationid"
            | b"runtimerunid"
    ))
}

pub(crate) struct EphemeralCredential(HeaderValue);

impl EphemeralCredential {
    pub(crate) fn from_environment() -> Result<Self, SessionFailureKind> {
        let token =
            std::env::var("LARM_API_TOKEN").map_err(|_| SessionFailureKind::Authentication)?;
        Self::from_token(&token)
    }

    fn from_token(token: &str) -> Result<Self, SessionFailureKind> {
        if token.is_empty()
            || token.len() > 4_096
            || token.bytes().any(|byte| byte.is_ascii_whitespace())
        {
            return Err(SessionFailureKind::Authentication);
        }
        let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| SessionFailureKind::Authentication)?;
        value.set_sensitive(true);
        Ok(Self(value))
    }

    #[cfg(test)]
    pub(crate) fn fixture() -> Self {
        let mut value = HeaderValue::from_static("Bearer fixture-token");
        value.set_sensitive(true);
        Self(value)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Cancellation<'a> {
    pub(crate) flag: &'a AtomicBool,
    pub(crate) notify: &'a Notify,
}

impl Cancellation<'_> {
    pub(crate) fn is_cancelled(self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub(crate) async fn cancelled(self) {
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

pub(crate) struct LarmHttpClient<'a> {
    client: &'a reqwest::Client,
    base_url: Url,
    credential: Option<&'a EphemeralCredential>,
    ttl_seconds: u32,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum AllocationStart {
    Ready(ReadyAllocation),
    Pending(PendingAllocation),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum OperationProgress {
    Pending,
    Succeeded,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum CleanupResult {
    Released,
    DeferredToTtl(ReleaseFailureKind),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ChatMessage {
    pub(crate) role: &'static str,
    pub(crate) content: String,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct ChatCompletion {
    pub(crate) content: String,
    pub(crate) tool_call: Option<AgentToolCall>,
    pub(crate) request_id: Option<BoundedIdentifier>,
    pub(crate) renewed_allocation: ReadyAllocation,
    pub(crate) lease_received_at: tokio::time::Instant,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct LarmError {
    pub(crate) kind: SessionFailureKind,
    pub(crate) output_started: bool,
}

impl LarmError {
    pub(crate) fn new(kind: SessionFailureKind, output_started: bool) -> Self {
        Self {
            kind,
            output_started,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AllocationRequest<'a> {
    requirements: [AllocationRequirement<'a>; 1],
    client: &'static str,
    allow_fallback: bool,
    ttl_seconds: u32,
    deployment_policy: &'static str,
}

#[derive(Serialize)]
struct AllocationRequirement<'a> {
    capability: &'a str,
    route: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RenewRequest {
    ttl_seconds: u32,
}

impl Serialize for ChatMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("ChatMessage", 2)?;
        state.serialize_field("role", self.role)?;
        state.serialize_field("content", &self.content)?;
        state.end()
    }
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorValue,
}

#[derive(Deserialize)]
struct ErrorValue {
    code: String,
    #[allow(dead_code)]
    message: String,
}

impl<'a> LarmHttpClient<'a> {
    pub(crate) fn new_probe(
        shared: &'a SharedLarmClient,
        base_url: &str,
    ) -> Result<Self, SessionFailureKind> {
        let base_url = parse_base_url(base_url)?;
        Ok(Self {
            client: &shared.0,
            base_url,
            credential: None,
            ttl_seconds: 300,
        })
    }

    pub(crate) fn new(
        shared: &'a SharedLarmClient,
        base_url: &str,
        credential: &'a EphemeralCredential,
        ttl_seconds: u32,
    ) -> Result<Self, SessionFailureKind> {
        if !(60..=3_600).contains(&ttl_seconds) {
            return Err(SessionFailureKind::Contract);
        }
        let base_url = parse_base_url(base_url)?;
        Ok(Self {
            client: &shared.0,
            base_url,
            credential: Some(credential),
            ttl_seconds,
        })
    }

    pub(crate) async fn probe(&self) -> Result<(), SessionFailureKind> {
        self.probe_endpoint("health", "ok").await?;
        self.probe_endpoint("ready", "ready").await?;
        Ok(())
    }

    async fn probe_endpoint(
        &self,
        path: &'static str,
        expected: &'static str,
    ) -> Result<(), SessionFailureKind> {
        let response = self
            .client
            .get(self.url(path)?)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await
            .map_err(|error| classify_transport(&error))?;
        if !response.status().is_success() {
            return Err(classify_probe_response(response).await);
        }
        let body = read_body_limited(response, PROBE_BODY_LIMIT, None, false)
            .await
            .map_err(|error| error.kind)?;
        let value: Value =
            serde_json::from_slice(&body).map_err(|_| SessionFailureKind::Protocol)?;
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .ok_or(SessionFailureKind::Protocol)?;
        if status != expected {
            return Err(SessionFailureKind::Unavailable);
        }
        Ok(())
    }

    pub(crate) async fn allocate(
        &self,
        cancellation: Cancellation<'_>,
    ) -> Result<AllocationStart, LarmError> {
        let request = AllocationRequest {
            requirements: [AllocationRequirement {
                capability: CAPABILITY,
                route: ROUTE,
            }],
            client: "saaa-desktop",
            allow_fallback: false,
            ttl_seconds: self.ttl_seconds,
            deployment_policy: "existing-only",
        };
        let body = serde_json::to_vec(&request)
            .map_err(|_| LarmError::new(SessionFailureKind::Internal, false))?;
        let response = self
            .send(
                Method::POST,
                "v1/allocations",
                Some(&body),
                CONTROL_TIMEOUT,
                cancellation,
                false,
            )
            .await
            .map_err(|mut error| {
                if matches!(
                    error.kind,
                    SessionFailureKind::Network | SessionFailureKind::Timeout
                ) {
                    error.kind = SessionFailureKind::AllocationOutcomeUnknown;
                }
                error
            })?;
        let status = response.status();
        if status != StatusCode::OK && status != StatusCode::ACCEPTED {
            return Err(LarmError::new(
                classify_error_response(response, ERROR_BODY_LIMIT).await,
                false,
            ));
        }
        let dto: AllocationDto =
            decode_json_limited(response, CONTROL_BODY_LIMIT, cancellation, false).await?;
        if status == StatusCode::OK {
            Ok(AllocationStart::Ready(self.ready_allocation(dto)?))
        } else {
            Ok(AllocationStart::Pending(self.pending_allocation(dto)?))
        }
    }

    pub(crate) async fn get_operation(
        &self,
        operation_id: &BoundedIdentifier,
        allocation_id: Option<&BoundedIdentifier>,
        cancellation: Cancellation<'_>,
    ) -> Result<OperationProgress, LarmError> {
        let response = self
            .send(
                Method::GET,
                &format!("v1/operations/{}", operation_id.as_str()),
                None,
                CONTROL_TIMEOUT,
                cancellation,
                false,
            )
            .await?;
        if !response.status().is_success() {
            return Err(LarmError::new(
                classify_error_response(response, ERROR_BODY_LIMIT).await,
                false,
            ));
        }
        let operation: OperationDto =
            decode_json_limited(response, CONTROL_BODY_LIMIT, cancellation, false).await?;
        self.validate_operation(&operation, operation_id, allocation_id)?;
        match operation.status {
            OperationStatus::Pending | OperationStatus::Running => Ok(OperationProgress::Pending),
            OperationStatus::Succeeded if operation.ready => Ok(OperationProgress::Succeeded),
            OperationStatus::Succeeded => Err(LarmError::new(SessionFailureKind::Protocol, false)),
            OperationStatus::Failed | OperationStatus::Cancelled | OperationStatus::TimedOut => {
                Err(LarmError::new(operation_failure_kind(&operation), false))
            }
        }
    }

    pub(crate) async fn get_allocation(
        &self,
        allocation_id: &BoundedIdentifier,
        cancellation: Cancellation<'_>,
    ) -> Result<ReadyAllocation, LarmError> {
        let response = self
            .send(
                Method::GET,
                &format!("v1/allocations/{}", allocation_id.as_str()),
                None,
                CONTROL_TIMEOUT,
                cancellation,
                false,
            )
            .await?;
        if !response.status().is_success() {
            return Err(LarmError::new(
                classify_error_response(response, ERROR_BODY_LIMIT).await,
                false,
            ));
        }
        let dto: AllocationDto =
            decode_json_limited(response, CONTROL_BODY_LIMIT, cancellation, false).await?;
        let ready = self.ready_allocation(dto)?;
        if &ready.allocation_id != allocation_id {
            return Err(LarmError::new(SessionFailureKind::Protocol, false));
        }
        Ok(ready)
    }

    pub(crate) async fn renew(
        &self,
        allocation: &ReadyAllocation,
        cancellation: Cancellation<'_>,
        output_started: bool,
    ) -> Result<ReadyAllocation, LarmError> {
        let body = serde_json::to_vec(&RenewRequest {
            ttl_seconds: self.ttl_seconds,
        })
        .map_err(|_| LarmError::new(SessionFailureKind::Internal, output_started))?;
        let response = self
            .send(
                Method::POST,
                &format!("v1/allocations/{}/renew", allocation.allocation_id.as_str()),
                Some(&body),
                CONTROL_TIMEOUT,
                cancellation,
                output_started,
            )
            .await?;
        if !response.status().is_success() {
            return Err(LarmError::new(
                classify_error_response(response, ERROR_BODY_LIMIT).await,
                output_started,
            ));
        }
        let dto: AllocationDto =
            decode_json_limited(response, CONTROL_BODY_LIMIT, cancellation, output_started).await?;
        let renewed = self.ready_allocation(dto)?;
        if !allocation.same_binding_as(&renewed) {
            return Err(LarmError::new(SessionFailureKind::Protocol, output_started));
        }
        Ok(renewed)
    }

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
            "reasoning_effort": reasoning_effort
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
        if tool_call.is_some() && !content.trim().is_empty() {
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

    pub(crate) async fn release(&self, allocation_id: &BoundedIdentifier) -> CleanupResult {
        let release_flag = AtomicBool::new(false);
        let release_notify = Notify::new();
        let cancellation = Cancellation {
            flag: &release_flag,
            notify: &release_notify,
        };
        let response = self
            .send(
                Method::DELETE,
                &format!("v1/allocations/{}", allocation_id.as_str()),
                None,
                RELEASE_TIMEOUT,
                cancellation,
                false,
            )
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                match read_body_limited(response, RELEASE_BODY_LIMIT, None, false).await {
                    Ok(_) => CleanupResult::Released,
                    Err(error) => CleanupResult::DeferredToTtl(release_kind(error.kind)),
                }
            }
            Ok(response) if response.status() == StatusCode::NOT_FOUND => {
                let kind = classify_error_response(response, ERROR_BODY_LIMIT).await;
                if kind == SessionFailureKind::AllocationLost {
                    CleanupResult::Released
                } else {
                    CleanupResult::DeferredToTtl(release_kind(kind))
                }
            }
            Ok(response) => CleanupResult::DeferredToTtl(release_kind(
                classify_error_response(response, ERROR_BODY_LIMIT).await,
            )),
            Err(error) => CleanupResult::DeferredToTtl(release_kind(error.kind)),
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&[u8]>,
        timeout: Duration,
        cancellation: Cancellation<'_>,
        output_started: bool,
    ) -> Result<reqwest::Response, LarmError> {
        if cancellation.is_cancelled() {
            return Err(LarmError::new(
                SessionFailureKind::Cancelled,
                output_started,
            ));
        }
        let credential = self
            .credential
            .ok_or_else(|| LarmError::new(SessionFailureKind::Authentication, output_started))?;
        let mut request = self
            .client
            .request(
                method,
                self.url(path)
                    .map_err(|kind| LarmError::new(kind, output_started))?,
            )
            .header(AUTHORIZATION, credential.0.clone())
            .timeout(timeout);
        if let Some(body) = body {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .body(body.to_vec());
        }
        tokio::select! {
            _ = cancellation.cancelled() => Err(LarmError::new(SessionFailureKind::Cancelled, output_started)),
            response = request.send() => response
                .map_err(|error| LarmError::new(classify_transport(&error), output_started)),
        }
    }

    async fn send_with_allocation(
        &self,
        body: &[u8],
        allocation_id: &str,
        timeout: Duration,
        cancellation: Cancellation<'_>,
    ) -> Result<reqwest::Response, LarmError> {
        if cancellation.is_cancelled() {
            return Err(LarmError::new(SessionFailureKind::Cancelled, false));
        }
        let credential = self
            .credential
            .ok_or_else(|| LarmError::new(SessionFailureKind::Authentication, false))?;
        let request = self
            .client
            .post(
                self.url("v1/chat/completions")
                    .map_err(|kind| LarmError::new(kind, false))?,
            )
            .header(AUTHORIZATION, credential.0.clone())
            .header("x-larm-allocation-id", allocation_id)
            .header(CONTENT_TYPE, "application/json")
            .timeout(timeout)
            .body(body.to_vec());
        tokio::select! {
            _ = cancellation.cancelled() => Err(LarmError::new(SessionFailureKind::Cancelled, false)),
            response = request.send() => response.map_err(|error| LarmError::new(classify_transport(&error), false)),
        }
    }

    fn url(&self, path: &str) -> Result<Url, SessionFailureKind> {
        self.base_url
            .join(path)
            .map_err(|_| SessionFailureKind::Contract)
    }

    fn ready_allocation(&self, dto: AllocationDto) -> Result<ReadyAllocation, LarmError> {
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

    fn pending_allocation(&self, dto: AllocationDto) -> Result<PendingAllocation, LarmError> {
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

    fn validate_operation(
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

fn validate_allocation_common(dto: &AllocationDto) -> Result<(), LarmError> {
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

fn valid_external_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn valid_datetime(value: &str) -> bool {
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

fn operation_failure_kind(operation: &OperationDto) -> SessionFailureKind {
    operation
        .error
        .as_ref()
        .map(|error| classify_operation_error_code(&error.code))
        .unwrap_or(SessionFailureKind::Unavailable)
}

fn classify_operation_error_code(code: &str) -> SessionFailureKind {
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

fn binding_fingerprint(binding: &super::contracts::AllocationBindingDto) -> String {
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

fn binding_identity(binding: &super::contracts::AllocationBindingDto) -> String {
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

fn parse_base_url(value: &str) -> Result<Url, SessionFailureKind> {
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

fn media_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

async fn decode_json_limited<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
    limit: usize,
    cancellation: Cancellation<'_>,
    output_started: bool,
) -> Result<T, LarmError> {
    let body = read_body_limited(response, limit, Some(cancellation), output_started).await?;
    serde_json::from_slice(&body)
        .map_err(|_| LarmError::new(SessionFailureKind::Protocol, output_started))
}

async fn read_body_limited(
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

async fn classify_error_response(response: reqwest::Response, limit: usize) -> SessionFailureKind {
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

async fn classify_probe_response(response: reqwest::Response) -> SessionFailureKind {
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

fn classify_error_code(code: &str, status: StatusCode) -> SessionFailureKind {
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

fn classify_transport(error: &reqwest::Error) -> SessionFailureKind {
    if error.is_timeout() {
        SessionFailureKind::Timeout
    } else if error.is_builder() {
        SessionFailureKind::Internal
    } else {
        SessionFailureKind::Network
    }
}

fn should_retry_stream_renew(kind: SessionFailureKind, previous_retries: u8) -> bool {
    kind.permits_stream_renew_retry() && previous_retries == 0
}

fn lease_deadlines(
    received_at: tokio::time::Instant,
    effective_ttl_seconds: u32,
) -> (Option<tokio::time::Instant>, tokio::time::Instant) {
    let ttl_seconds = u64::from(effective_ttl_seconds);
    let ttl = Duration::from_secs(ttl_seconds);
    let renew_after = Duration::from_secs(ttl_seconds * 4 / 5);
    (Some(received_at + renew_after), received_at + ttl)
}

fn release_kind(kind: SessionFailureKind) -> ReleaseFailureKind {
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

fn drain_sse(buffer: &mut Vec<u8>, output_started: bool) -> Result<Vec<String>, LarmError> {
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

fn project_sse<F>(
    events: Vec<String>,
    content: &mut String,
    content_chars: &mut usize,
    output_started: &mut bool,
    on_delta: &mut F,
    tool_calls: &mut ToolCallAccumulator,
) -> Result<bool, LarmError>
where
    F: FnMut(&str, bool) -> Result<(), SessionFailureKind>,
{
    for event in events {
        for line in event.lines().filter_map(|line| line.strip_prefix("data:")) {
            let data = line.trim();
            if data == "[DONE]" {
                return Ok(true);
            }
            let value: Value = serde_json::from_str(data)
                .map_err(|_| LarmError::new(SessionFailureKind::Protocol, *output_started))?;
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
    }
    Ok(false)
}

fn tool_protocol_error(error: ToolProtocolError, output_started: bool) -> LarmError {
    let kind = match error {
        ToolProtocolError::Protocol => SessionFailureKind::Protocol,
        ToolProtocolError::TooLarge => SessionFailureKind::RequestTooLarge,
    };
    LarmError::new(kind, output_started)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        io::{ErrorKind, Read, Write},
        net::{TcpListener, TcpStream},
        sync::{atomic::AtomicBool, Arc, Mutex},
        thread,
    };

    struct FakeServer {
        base_url: String,
        captures: Arc<Mutex<Vec<Vec<u8>>>>,
        stop: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl FakeServer {
        fn start(responses: Vec<Vec<u8>>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("fake LARM binds");
            listener
                .set_nonblocking(true)
                .expect("fake LARM becomes nonblocking");
            let address = listener.local_addr().expect("fake LARM address");
            let captures = Arc::new(Mutex::new(Vec::new()));
            let thread_captures = captures.clone();
            let stop = Arc::new(AtomicBool::new(false));
            let thread_stop = stop.clone();
            let handle = thread::spawn(move || {
                let mut responses = VecDeque::from(responses);
                while !thread_stop.load(Ordering::SeqCst) && !responses.is_empty() {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream
                                .set_nonblocking(false)
                                .expect("fake LARM stream becomes blocking");
                            let request = read_request(&mut stream);
                            thread_captures.lock().expect("capture lock").push(request);
                            if let Some(response) = responses.pop_front() {
                                let _ = stream.write_all(&response);
                                let _ = stream.flush();
                            }
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                base_url: format!("http://{address}/"),
                captures,
                stop,
                handle: Some(handle),
            }
        }

        fn captures(&self) -> Vec<String> {
            self.captures
                .lock()
                .expect("capture lock")
                .iter()
                .map(|capture| String::from_utf8_lossy(capture).into_owned())
                .collect()
        }
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Ok(url) = Url::parse(&self.base_url) {
                if let (Some(host), Some(port)) = (url.host_str(), url.port()) {
                    let _ = TcpStream::connect((host, port));
                }
            }
            if let Some(handle) = self.handle.take() {
                handle.join().expect("fake LARM stops");
            }
        }
    }

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut content_length = None;
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    request.extend_from_slice(&buffer[..count]);
                    if content_length.is_none() {
                        content_length = request
                            .windows(4)
                            .position(|window| window == b"\r\n\r\n")
                            .map(|header_end| {
                                let headers = String::from_utf8_lossy(&request[..header_end]);
                                let length = headers.lines().find_map(|line| {
                                    let (name, value) = line.split_once(':')?;
                                    name.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().ok())
                                        .flatten()
                                });
                                (header_end + 4, length.unwrap_or(0))
                            });
                    }
                    if content_length.is_some_and(|(header_end, length)| {
                        request.len() >= header_end.saturating_add(length)
                    }) {
                        break;
                    }
                    if request.len() > GATEWAY_REQUEST_LIMIT + 16 * 1_024 {
                        break;
                    }
                }
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    break
                }
                Err(_) => break,
            }
        }
        request
    }

    fn response(status: &str, content_type: &str, body: &str) -> Vec<u8> {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes()
    }

    fn ready_json(id: &str, runtime: &str, fallback: bool, reason: &str) -> String {
        format!(
            r#"{{"id":"{id}","status":"ready","requirements":[{{"capability":"llm.general","route":"llm-default"}}],"bindings":[{{"capability":"llm.general","route":"llm-default","runtime":"{runtime}","node":"gnosis","status":"HOT","candidateRank":1,"fallback":{fallback},"selectionReason":"{reason}"}}],"allowFallback":false,"deploymentPolicy":"existing-only","createdAt":"2026-08-28T00:00:00.000Z","expiresAt":"2026-08-28T00:05:00.000Z"}}"#
        )
    }

    fn credential() -> EphemeralCredential {
        let mut value = HeaderValue::from_static("Bearer test-token");
        value.set_sensitive(true);
        EphemeralCredential(value)
    }

    fn cancellation() -> (AtomicBool, Notify) {
        (AtomicBool::new(false), Notify::new())
    }

    struct EnvironmentGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvironmentGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvironmentGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[tokio::test]
    async fn canary_authentication_and_metrics_helpers_are_strict_and_bounded() {
        let _environment_lock = super::super::test_environment_lock().lock().await;
        let _guard = EnvironmentGuard::set("LARM_API_TOKEN", "test-token");
        let _scope_guard = EnvironmentGuard::set("SAAA_LARM_CANARY_METRICS_SCOPE", "client-scoped");
        let server = FakeServer::start(vec![
            response(
                "401 Unauthorized",
                "application/json",
                r#"{"error":{"code":"unauthorized","message":"denied"}}"#,
            ),
            response(
                "401 Unauthorized",
                "application/json",
                r#"{"error":{"code":"unauthorized","message":"denied"}}"#,
            ),
            response(
                "404 Not Found",
                "application/json",
                r#"{"error":{"code":"not_found","message":"missing"}}"#,
            ),
            response(
                "200 OK",
                "text/plain; version=0.0.4",
                "# TYPE larm_active_allocations gauge\nhttp_response_seconds 1\nlarm_active_allocations{client=\"saaa-desktop\"} 0\n",
            ),
        ]);
        let shared = SharedLarmClient::build().expect("shared client builds");
        shared
            .canary_authentication_boundary(&server.base_url)
            .await
            .expect("auth boundary validates");
        assert_eq!(
            shared
                .canary_active_allocations(&server.base_url)
                .await
                .expect("metrics validate"),
            0
        );
        let captures = server.captures();
        assert_eq!(captures.len(), 4);
        assert!(!captures[0].to_ascii_lowercase().contains("authorization:"));
        assert!(captures[1].contains("authorization: Bearer "));
        assert!(captures[2].contains("authorization: Bearer test-token"));
        assert!(captures[3].contains("GET /metrics HTTP/1.1"));

        let leaking = FakeServer::start(vec![response(
            "200 OK",
            "text/plain",
            "leaking_metric{request_id=\"private\"} 1\nlarm_active_allocations 0\n",
        )]);
        assert_eq!(
            shared.canary_active_allocations(&leaking.base_url).await,
            Err(SessionFailureKind::Policy)
        );

        let camel_case_leak = FakeServer::start(vec![response(
            "200 OK",
            "text/plain",
            "unrelated_metric{allocationId=\"private\"} 1\nlarm_active_allocations{client=\"saaa-desktop\"} 0\n",
        )]);
        assert_eq!(
            shared
                .canary_active_allocations(&camel_case_leak.base_url)
                .await,
            Err(SessionFailureKind::Policy)
        );

        let wrong_scope = FakeServer::start(vec![response(
            "200 OK",
            "text/plain",
            "larm_active_allocations 0\n",
        )]);
        assert_eq!(
            shared
                .canary_active_allocations(&wrong_scope.base_url)
                .await,
            Err(SessionFailureKind::Contract)
        );

        let restarting = FakeServer::start(vec![response(
            "503 Service Unavailable",
            "application/json",
            r#"{"status":"restarting"}"#,
        )]);
        assert_eq!(
            shared.canary_active_allocations(&restarting.base_url).await,
            Err(SessionFailureKind::Upstream)
        );

        {
            let _exclusive =
                EnvironmentGuard::set("SAAA_LARM_CANARY_METRICS_SCOPE", "exclusive-window");
            let exclusive = FakeServer::start(vec![response(
                "200 OK",
                "text/plain",
                "larm_active_allocations 0\n",
            )]);
            assert_eq!(
                shared
                    .canary_active_allocations(&exclusive.base_url)
                    .await
                    .expect("exclusive metric validates"),
                0
            );
        }
    }

    #[test]
    fn credential_and_base_url_reject_ambiguous_inputs() {
        for token in ["", "two words", "tab\ttoken", "line\ntoken"] {
            assert_eq!(
                EphemeralCredential::from_token(token).err(),
                Some(SessionFailureKind::Authentication)
            );
        }
        assert!(EphemeralCredential::from_token("fixture-token").is_ok());

        assert!(parse_base_url("http://127.0.0.1:9810/").is_ok());
        assert!(parse_base_url("http://[::1]:9810/").is_ok());
        for url in [
            "http://localhost:9810/",
            "https://127.0.0.1:9810/",
            "http://127.0.0.1:9810/v1",
            "http://user:secret@127.0.0.1:9810/",
            "http://127.0.0.1/",
        ] {
            assert_eq!(parse_base_url(url), Err(SessionFailureKind::Contract));
        }
    }

    #[tokio::test]
    async fn ready_stream_release_uses_fixed_contract_and_bounded_headers() {
        let allocation = ready_json("alloc_1", "runtime_1", false, "primary-live");
        let stream =
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\ndata: [DONE]\n\n";
        let mut chat_response = response("200 OK", "text/event-stream", stream);
        let header_end = chat_response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("header boundary");
        chat_response.splice(
            header_end..header_end,
            b"\r\nX-Request-ID: req_1".iter().copied(),
        );
        let server = FakeServer::start(vec![
            response("200 OK", "application/json", &allocation),
            chat_response,
            response("200 OK", "application/json", &allocation),
        ]);
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, &server.base_url, &credential, 300)
            .expect("client config");
        let (flag, notify) = cancellation();
        let cancellation = Cancellation {
            flag: &flag,
            notify: &notify,
        };
        let allocation = match client
            .allocate(cancellation)
            .await
            .expect("allocation ready")
        {
            AllocationStart::Ready(allocation) => allocation,
            AllocationStart::Pending(_) => panic!("unexpected pending allocation"),
        };
        let mut deltas = Vec::new();
        let completion = client
            .chat(
                &allocation,
                tokio::time::Instant::now(),
                &[ChatMessage {
                    role: "user",
                    content: "fixture prompt".to_string(),
                }],
                Duration::from_secs(5),
                cancellation,
                |delta, first| {
                    deltas.push((delta.to_string(), first));
                    Ok(())
                },
            )
            .await
            .expect("chat completes");
        assert_eq!(completion.content, "hello");
        assert_eq!(
            completion
                .request_id
                .as_ref()
                .map(BoundedIdentifier::as_str),
            Some("req_1")
        );
        assert_eq!(deltas, vec![("hello".to_string(), true)]);
        assert_eq!(
            client.release(&allocation.allocation_id).await,
            CleanupResult::Released
        );

        let captures = server.captures();
        assert_eq!(captures.len(), 3);
        assert!(
            captures[0].contains("POST /v1/allocations HTTP/1.1"),
            "unexpected allocation request: {:?}",
            captures[0]
        );
        assert!(captures[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer test-token"));
        assert!(captures[0].contains("\"allowFallback\":false"));
        assert!(captures[0].contains("\"deploymentPolicy\":\"existing-only\""));
        assert!(captures[1].contains("x-larm-allocation-id: alloc_1"));
        assert!(captures[1].contains("\"model\":\"local\""));
        assert!(captures[1].contains("\"reasoning_effort\":\"medium\""));
        assert!(captures[2].contains("DELETE /v1/allocations/alloc_1 HTTP/1.1"));
    }

    #[tokio::test]
    async fn larm_gateway_projects_recall_tool_calls_and_serializes_tool_exchanges() {
        let first = serde_json::json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call_larm_recall",
                "function": {
                    "name": "recall_conversation",
                    "arguments": "{\"time\":{\"kind\":\"preset\","
                }
            }]}}]
        });
        let second = serde_json::json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "function": {"arguments": "\"preset\":\"yesterday\"}}"}
            }]}}]
        });
        let stream = format!("data: {first}\n\ndata: {second}\n\ndata: [DONE]\n\n");
        let server = FakeServer::start(vec![response("200 OK", "text/event-stream", &stream)]);
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, &server.base_url, &credential, 300)
            .expect("client config");
        let allocation = client
            .ready_allocation(
                serde_json::from_str(&ready_json(
                    "alloc_tool",
                    "runtime_tool",
                    false,
                    "primary-live",
                ))
                .expect("fixture decodes"),
            )
            .expect("allocation validates");
        let (flag, notify) = cancellation();
        let tool_exchange = serde_json::json!({
            "role": "tool",
            "tool_call_id": "call_previous",
            "content": "{\"reasonCode\":\"continuity-no-hit\"}"
        });
        let tools = [crate::runtime::agent_tools::recall_tool_definition()];
        let completion = client
            .chat_with_tools(
                &allocation,
                tokio::time::Instant::now(),
                &[ChatMessage {
                    role: "user",
                    content: "昨日の話".to_string(),
                }],
                &[tool_exchange],
                &tools,
                "xhigh",
                Duration::from_secs(5),
                Cancellation {
                    flag: &flag,
                    notify: &notify,
                },
                |_, _| Ok(()),
            )
            .await
            .expect("tool call completes");
        assert!(completion.content.is_empty());
        let call = completion.tool_call.expect("tool call projects");
        assert_eq!(call.id, "call_larm_recall");
        assert_eq!(call.name, "recall_conversation");
        assert_eq!(
            crate::runtime::agent_tools::parse_recall_arguments(&call.arguments)
                .expect("arguments decode")
                .time,
            Some(crate::memory::contracts::RecallTimeFilter::Preset {
                preset: crate::memory::contracts::RecallTimePreset::Yesterday
            })
        );

        let captures = server.captures();
        assert_eq!(captures.len(), 1);
        assert!(captures[0].contains("\"name\":\"recall_conversation\""));
        assert!(captures[0].contains("\"tool_choice\":\"auto\""));
        assert!(captures[0].contains("\"tool_call_id\":\"call_previous\""));
        assert!(captures[0].contains("\"reasoning_effort\":\"xhigh\""));
    }

    #[tokio::test]
    async fn larm_stream_disconnect_after_delta_is_marked_partial_and_never_done() {
        let partial = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
        let server = FakeServer::start(vec![response("200 OK", "text/event-stream", partial)]);
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, &server.base_url, &credential, 300)
            .expect("client config");
        let allocation_dto: AllocationDto = serde_json::from_str(&ready_json(
            "alloc_partial",
            "runtime_partial",
            false,
            "primary-live",
        ))
        .expect("fixture decodes");
        let allocation = client
            .ready_allocation(allocation_dto)
            .expect("allocation validates");
        let (flag, notify) = cancellation();
        let mut deltas = Vec::new();
        let error = client
            .chat(
                &allocation,
                tokio::time::Instant::now(),
                &[ChatMessage {
                    role: "user",
                    content: "fixture".to_string(),
                }],
                Duration::from_secs(5),
                Cancellation {
                    flag: &flag,
                    notify: &notify,
                },
                |delta, _| {
                    deltas.push(delta.to_string());
                    Ok(())
                },
            )
            .await
            .expect_err("missing DONE is a partial failure");
        assert_eq!(deltas, vec!["partial"]);
        assert_eq!(error.kind, SessionFailureKind::Network);
        assert!(error.output_started);
    }

    #[tokio::test]
    async fn gateway_rejects_an_unknown_content_type() {
        let server = FakeServer::start(vec![response("200 OK", "text/plain", "data: [DONE]\n\n")]);
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, &server.base_url, &credential, 300)
            .expect("client config");
        let allocation = client
            .ready_allocation(
                serde_json::from_str(&ready_json(
                    "alloc_content_type",
                    "runtime_content_type",
                    false,
                    "primary-live",
                ))
                .expect("fixture decodes"),
            )
            .expect("allocation validates");
        let (flag, notify) = cancellation();
        let error = client
            .chat(
                &allocation,
                tokio::time::Instant::now(),
                &[ChatMessage {
                    role: "user",
                    content: "fixture".to_string(),
                }],
                Duration::from_secs(5),
                Cancellation {
                    flag: &flag,
                    notify: &notify,
                },
                |_, _| Ok(()),
            )
            .await
            .expect_err("unknown content type is rejected");
        assert_eq!(error.kind, SessionFailureKind::Protocol);
        assert!(!error.output_started);
    }

    #[tokio::test]
    async fn gateway_rejects_non_stream_success_and_expired_lease_before_network() {
        let json = r#"{"choices":[{"message":{"content":"must not be accepted"}}]}"#;
        let server = FakeServer::start(vec![response("200 OK", "application/json", json)]);
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, &server.base_url, &credential, 300)
            .expect("client config");
        let allocation = client
            .ready_allocation(
                serde_json::from_str(&ready_json(
                    "alloc_non_stream",
                    "runtime_non_stream",
                    false,
                    "primary-live",
                ))
                .expect("fixture decodes"),
            )
            .expect("allocation validates");
        let (flag, notify) = cancellation();
        let cancellation = Cancellation {
            flag: &flag,
            notify: &notify,
        };
        let error = client
            .chat(
                &allocation,
                tokio::time::Instant::now(),
                &[ChatMessage {
                    role: "user",
                    content: "fixture".to_string(),
                }],
                Duration::from_secs(5),
                cancellation,
                |_, _| Ok(()),
            )
            .await
            .expect_err("stream request rejects a non-stream success response");
        assert_eq!(error.kind, SessionFailureKind::Protocol);

        let expired = tokio::time::Instant::now() - Duration::from_secs(301);
        let error = client
            .chat(
                &allocation,
                expired,
                &[ChatMessage {
                    role: "user",
                    content: "fixture".to_string(),
                }],
                Duration::from_secs(5),
                cancellation,
                |_, _| Ok(()),
            )
            .await
            .expect_err("expired allocation is rejected before a second request");
        assert_eq!(error.kind, SessionFailureKind::AllocationLost);
        assert_eq!(server.captures().len(), 1);
    }

    #[test]
    fn lease_schedule_is_anchored_to_allocation_response_receipt() {
        let received_at = tokio::time::Instant::now();
        let (renew_at, expires_at) = lease_deadlines(received_at, 60);
        assert_eq!(renew_at, Some(received_at + Duration::from_secs(48)));
        assert_eq!(expires_at, received_at + Duration::from_secs(60));
    }

    #[tokio::test]
    async fn pending_operation_can_be_polled_to_the_same_ready_allocation() {
        let pending = ready_json("alloc_2", "runtime_2", false, "primary-live")
            .replace("\"status\":\"ready\"", "\"status\":\"pending\"")
            .replace(
                "\"expiresAt\":\"2026-08-28T00:05:00.000Z\"",
                "\"expiresAt\":\"2026-08-28T00:05:00.000Z\",\"operationId\":\"op_2\"",
            );
        let operation = r#"{"id":"op_2","kind":"allocation","allocationId":"alloc_2","status":"succeeded","ready":true,"desired":["runtime_2"],"ensure":[],"createdAt":"2026-08-28T00:00:00.000Z"}"#;
        let ready = ready_json("alloc_2", "runtime_2", false, "primary-live");
        let server = FakeServer::start(vec![
            response("202 Accepted", "application/json", &pending),
            response("200 OK", "application/json", operation),
            response("200 OK", "application/json", &ready),
        ]);
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, &server.base_url, &credential, 300)
            .expect("client config");
        let (flag, notify) = cancellation();
        let cancellation = Cancellation {
            flag: &flag,
            notify: &notify,
        };
        let pending = match client
            .allocate(cancellation)
            .await
            .expect("pending accepted")
        {
            AllocationStart::Pending(pending) => pending,
            AllocationStart::Ready(_) => panic!("unexpected ready allocation"),
        };
        assert_eq!(
            client
                .get_operation(
                    &pending.operation_id,
                    pending.cleanup_allocation_id.as_ref(),
                    cancellation,
                )
                .await
                .expect("operation succeeds"),
            OperationProgress::Succeeded
        );
        let ready = client
            .get_allocation(
                pending
                    .cleanup_allocation_id
                    .as_ref()
                    .expect("pending includes allocation id"),
                cancellation,
            )
            .await
            .expect("allocation becomes ready");
        assert_eq!(ready.allocation_id.as_str(), "alloc_2");
    }

    #[tokio::test]
    async fn probe_is_public_and_redirects_are_not_followed() {
        let body = r#"{"error":{"code":"bad_request","message":"x"}}"#;
        let redirect = format!(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/health\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .into_bytes();
        let server = FakeServer::start(vec![redirect]);
        let shared = SharedLarmClient::build().expect("client builds");
        let client = LarmHttpClient::new_probe(&shared, &server.base_url).expect("probe config");
        assert_eq!(client.probe().await, Err(SessionFailureKind::Protocol));
        let captures = server.captures();
        assert_eq!(captures.len(), 1);
        assert!(!captures[0].to_ascii_lowercase().contains("authorization:"));
    }

    #[tokio::test]
    async fn readiness_stale_or_draining_is_bounded_unavailable() {
        let server = FakeServer::start(vec![
            response("200 OK", "application/json", r#"{"status":"ok"}"#),
            response(
                "503 Service Unavailable",
                "application/json",
                r#"{"status":"draining"}"#,
            ),
        ]);
        let shared = SharedLarmClient::build().expect("client builds");
        let client = LarmHttpClient::new_probe(&shared, &server.base_url).expect("probe config");
        assert_eq!(client.probe().await, Err(SessionFailureKind::Unavailable));
        assert!(server
            .captures()
            .iter()
            .all(|request| !request.to_ascii_lowercase().contains("authorization:")));
    }

    #[tokio::test]
    async fn oversized_control_response_is_rejected_without_copying_the_body() {
        let oversized = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 300000\r\nConnection: close\r\n\r\n{}".to_vec();
        let server = FakeServer::start(vec![oversized]);
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, &server.base_url, &credential, 300)
            .expect("client config");
        let (flag, notify) = cancellation();
        let error = client
            .allocate(Cancellation {
                flag: &flag,
                notify: &notify,
            })
            .await
            .expect_err("oversized response is rejected");
        assert_eq!(error.kind, SessionFailureKind::RequestTooLarge);
    }

    #[tokio::test]
    async fn oversized_sse_event_is_rejected_as_a_bounded_failure() {
        let event = format!("data: {}", "x".repeat(SSE_EVENT_LIMIT + 1));
        let server = FakeServer::start(vec![response("200 OK", "text/event-stream", &event)]);
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, &server.base_url, &credential, 300)
            .expect("client config");
        let allocation_dto: AllocationDto = serde_json::from_str(&ready_json(
            "alloc_oversized_event",
            "runtime_oversized_event",
            false,
            "primary-live",
        ))
        .expect("fixture decodes");
        let allocation = client
            .ready_allocation(allocation_dto)
            .expect("allocation validates");
        let (flag, notify) = cancellation();
        let error = client
            .chat(
                &allocation,
                tokio::time::Instant::now(),
                &[ChatMessage {
                    role: "user",
                    content: "fixture".to_string(),
                }],
                Duration::from_secs(5),
                Cancellation {
                    flag: &flag,
                    notify: &notify,
                },
                |_, _| Ok(()),
            )
            .await
            .expect_err("oversized event is rejected");
        assert_eq!(error.kind, SessionFailureKind::RequestTooLarge);
        assert!(!error.output_started);
    }

    #[tokio::test]
    async fn release_not_found_is_idempotent_and_renew_cannot_change_binding() {
        let changed = ready_json("alloc_4", "runtime_changed", false, "primary-live");
        let not_found = r#"{"error":{"code":"not_found","message":"gone"}}"#;
        let server = FakeServer::start(vec![
            response("200 OK", "application/json", &changed),
            response("404 Not Found", "application/json", not_found),
        ]);
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, &server.base_url, &credential, 300)
            .expect("client config");
        let original_dto: AllocationDto = serde_json::from_str(&ready_json(
            "alloc_4",
            "runtime_original",
            false,
            "primary-live",
        ))
        .expect("fixture decodes");
        let original = client
            .ready_allocation(original_dto)
            .expect("original allocation validates");
        let (flag, notify) = cancellation();
        let error = client
            .renew(
                &original,
                Cancellation {
                    flag: &flag,
                    notify: &notify,
                },
                true,
            )
            .await
            .expect_err("binding change rejected");
        assert_eq!(error.kind, SessionFailureKind::Protocol);
        assert!(error.output_started);
        assert_eq!(
            client.release(&original.allocation_id).await,
            CleanupResult::Released
        );
    }

    #[test]
    fn ready_invariants_reject_fallback_and_normalize_unknown_reason() {
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, "http://127.0.0.1:9810/", &credential, 300)
            .expect("client config");
        let fallback: AllocationDto =
            serde_json::from_str(&ready_json("alloc_3", "runtime_3", true, "fallback"))
                .expect("fixture decodes");
        assert_eq!(
            client
                .ready_allocation(fallback)
                .expect_err("fallback rejected")
                .kind,
            SessionFailureKind::Protocol
        );
        let unknown: AllocationDto =
            serde_json::from_str(&ready_json("alloc_3", "runtime_3", false, "new-reason"))
                .expect("fixture decodes");
        assert_eq!(
            client
                .ready_allocation(unknown)
                .expect("unknown reason remains successful")
                .selection_reason,
            SelectionReason::Other
        );

        let mut invalid_optional_id: AllocationDto =
            serde_json::from_str(&ready_json("alloc_3", "runtime_3", false, "primary-live"))
                .expect("fixture decodes");
        invalid_optional_id.operation_id = Some("bad/operation".to_string());
        assert_eq!(
            client
                .ready_allocation(invalid_optional_id)
                .expect_err("optional identifiers remain bounded")
                .kind,
            SessionFailureKind::Protocol
        );

        let mut controlled_text: AllocationDto =
            serde_json::from_str(&ready_json("alloc_3", "runtime_3", false, "primary-live"))
                .expect("fixture decodes");
        controlled_text.bindings[0].selection_reason = "primary\nforged".to_string();
        assert_eq!(
            client
                .ready_allocation(controlled_text)
                .expect_err("control characters are rejected")
                .kind,
            SessionFailureKind::Protocol
        );
    }

    #[test]
    fn error_catalog_rejects_unknown_codes() {
        assert_eq!(
            classify_error_code("mystery", StatusCode::SERVICE_UNAVAILABLE),
            SessionFailureKind::Protocol
        );
        assert_eq!(
            classify_error_code("resource_exhausted", StatusCode::CONFLICT),
            SessionFailureKind::Capacity
        );
        assert_eq!(
            classify_operation_error_code("internal_error"),
            SessionFailureKind::Upstream
        );
        assert_eq!(
            classify_operation_error_code("no_candidate_available"),
            SessionFailureKind::Unavailable
        );
        assert_eq!(
            classify_operation_error_code("mystery"),
            SessionFailureKind::Protocol
        );
    }

    #[test]
    fn stream_renew_retry_is_independent_of_first_delta_timing() {
        for kind in [
            SessionFailureKind::Network,
            SessionFailureKind::Timeout,
            SessionFailureKind::Upstream,
        ] {
            assert!(should_retry_stream_renew(kind, 0));
            assert!(!should_retry_stream_renew(kind, 1));
        }
        for kind in [
            SessionFailureKind::Authentication,
            SessionFailureKind::Contract,
            SessionFailureKind::Protocol,
            SessionFailureKind::AllocationLost,
        ] {
            assert!(!should_retry_stream_renew(kind, 0));
        }
    }

    #[test]
    fn selection_reason_is_bounded_by_conversion() {
        let binding = super::super::contracts::AllocationBindingDto {
            capability: CAPABILITY.to_string(),
            route: ROUTE.to_string(),
            runtime: "runtime_1".to_string(),
            node: "node_1".to_string(),
            status: "HOT".to_string(),
            candidate_rank: 1,
            fallback: false,
            selection_reason: "a-new-server-reason".to_string(),
        };
        assert!(binding_fingerprint(&binding).starts_with("binding_"));
    }

    #[test]
    fn binding_invariance_uses_exact_identity_not_only_the_display_fingerprint() {
        let first = ReadyAllocation::new_with_binding_identity(
            "alloc_1",
            "runtime_1",
            "binding_same",
            "exact_identity_a",
            300,
            false,
            SelectionReason::Primary,
        )
        .expect("first allocation validates");
        let second = ReadyAllocation::new_with_binding_identity(
            "alloc_1",
            "runtime_1",
            "binding_same",
            "exact_identity_b",
            300,
            false,
            SelectionReason::Primary,
        )
        .expect("second allocation validates");
        assert!(!first.same_binding_as(&second));
    }

    #[test]
    fn sse_limit_is_enforced_per_event_and_callback_failure_is_partial() {
        let event = format!("data: {}\n\n", "x".repeat(600 * 1_024));
        let mut buffer = [event.as_bytes(), event.as_bytes()].concat();
        let events = drain_sse(&mut buffer, false).expect("bounded events drain");
        assert_eq!(events.len(), 2);
        assert!(buffer.is_empty());

        let mut content = String::new();
        let mut content_chars = 0;
        let mut output_started = false;
        let mut tool_calls = ToolCallAccumulator::default();
        let error = project_sse(
            vec!["data: {\"choices\":[{\"delta\":{\"content\":\"visible\"}}]}".to_string()],
            &mut content,
            &mut content_chars,
            &mut output_started,
            &mut |_, _| Err(SessionFailureKind::ClientDisconnected),
            &mut tool_calls,
        )
        .expect_err("consumer failure stops projection");
        assert_eq!(error.kind, SessionFailureKind::ClientDisconnected);
        assert!(error.output_started);
    }

    #[test]
    fn timestamp_contract_rejects_shape_and_range_errors() {
        assert!(valid_datetime("2026-08-28T00:00:00.000Z"));
        for value in [
            "TwhateverZ",
            "2026-13-28T00:00:00.000Z",
            "2026-08-00T00:00:00.000Z",
            "2026-02-29T00:00:00.000Z",
            "2026-04-31T00:00:00.000Z",
            "2026-08-28T24:00:00.000Z",
            "2026-08-28 00:00:00Z",
            "2026-08-28T00:00:00.Z",
            "2026-08-28T00:00:00.+00Z",
        ] {
            assert!(
                !valid_datetime(value),
                "invalid timestamp accepted: {value}"
            );
        }
    }

    #[tokio::test]
    async fn oversized_gateway_request_is_rejected_before_network() {
        let server = FakeServer::start(vec![response(
            "500 Internal Server Error",
            "application/json",
            r#"{"error":{"code":"internal_error","message":"must not be reached"}}"#,
        )]);
        let shared = SharedLarmClient::build().expect("client builds");
        let credential = credential();
        let client = LarmHttpClient::new(&shared, &server.base_url, &credential, 300)
            .expect("client config");
        let allocation_dto: AllocationDto = serde_json::from_str(&ready_json(
            "alloc_oversized_request",
            "runtime_oversized_request",
            false,
            "primary-live",
        ))
        .expect("fixture decodes");
        let allocation = client
            .ready_allocation(allocation_dto)
            .expect("allocation validates");
        let messages = [ChatMessage {
            role: "user",
            content: "x".repeat(GATEWAY_REQUEST_LIMIT),
        }];
        let (flag, notify) = cancellation();
        let error = client
            .chat(
                &allocation,
                tokio::time::Instant::now(),
                &messages,
                Duration::from_secs(5),
                Cancellation {
                    flag: &flag,
                    notify: &notify,
                },
                |_, _| Ok(()),
            )
            .await
            .expect_err("oversized request is rejected");
        assert_eq!(error.kind, SessionFailureKind::RequestTooLarge);
        assert!(server.captures().is_empty(), "request reached the network");
    }
}
