use super::contracts::{
    AllocationDto, BoundedIdentifier, OperationDto, OperationStatus, PendingAllocation,
    ReadyAllocation, ReleaseFailureKind, SessionFailureKind,
};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE},
    Method, StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::sync::Notify;
use url::Url;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(2);
const PROBE_BODY_LIMIT: usize = 64 * 1_024;
const CONTROL_BODY_LIMIT: usize = 256 * 1_024;
const RELEASE_BODY_LIMIT: usize = 64 * 1_024;
const ERROR_BODY_LIMIT: usize = 8 * 1_024;
const CAPABILITY: &str = "llm.general";
const ROUTE: &str = "llm-default";
mod decode;

pub(crate) use decode::*;

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

    pub(crate) fn authorization(&self) -> Result<&str, SessionFailureKind> {
        self.0
            .to_str()
            .map_err(|_| SessionFailureKind::Authentication)
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

#[cfg(test)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RenewRequest {
    ttl_seconds: u32,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorValue,
}

#[derive(Deserialize)]
struct ErrorValue {
    code: String,
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

    #[cfg(test)]
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

    fn url(&self, path: &str) -> Result<Url, SessionFailureKind> {
        self.base_url
            .join(path)
            .map_err(|_| SessionFailureKind::Contract)
    }
}

#[cfg(test)]
mod tests {
    use super::super::contracts::SelectionReason;
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
                    if request.len() > CONTROL_BODY_LIMIT + 16 * 1_024 {
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
            r#"{{"id":"{id}","status":"ready","requirements":[{{"capability":"llm.general","route":"llm-default"}}],"bindings":[{{"capability":"llm.general","route":"llm-default","runtime":"{runtime}","node":"dynamic-lan","status":"HOT","candidateRank":1,"fallback":{fallback},"selectionReason":"{reason}"}}],"allowFallback":false,"deploymentPolicy":"existing-only","createdAt":"2026-08-28T00:00:00.000Z","expiresAt":"2026-08-28T00:05:00.000Z"}}"#
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
    fn renewal_binding_invariance_uses_exact_identity() {
        let first = ReadyAllocation::new_with_binding_identity(
            "alloc_1",
            "runtime_1",
            "exact_identity_a",
            300,
            false,
            SelectionReason::Primary,
        )
        .expect("first allocation validates");
        let second = ReadyAllocation::new_with_binding_identity(
            "alloc_1",
            "runtime_1",
            "exact_identity_b",
            300,
            false,
            SelectionReason::Primary,
        )
        .expect("second allocation validates");
        assert!(!first.same_binding_as(&second));
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
}
