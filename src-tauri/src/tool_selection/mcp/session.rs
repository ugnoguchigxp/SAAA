//! Per-source MCP sessions and the lifecycle state machine.
//!
//! A session is owned by one (source, config generation) pair and is never shared with another
//! source. Initialize is single-flight per source: concurrent callers wait on the same state
//! mutex. A session id expiring (HTTP 404) moves the session to `Reconnecting` and the next
//! operation re-initializes once; an in-flight `tools/call` is never replayed.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};

use super::config::McpSourceSpec;
use super::transport::{HttpTransport, TransportError};
use super::{
    MCP_BACKOFF_SECONDS, MCP_CALLS_PER_PROFILE_MAX, MCP_CALLS_PER_SOURCE_MAX, MCP_CALL_QUEUE_MAX,
    MCP_CONNECT_TIMEOUT, MCP_INITIALIZE_TIMEOUT, MCP_PROTOCOL_VERSION, MCP_SHUTDOWN_GRACE,
};
use crate::RunCancellation;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Disabled,
    Initializing,
    Ready,
    Reconnecting,
    Unavailable,
    Closing,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Initializing => "initializing",
            Self::Ready => "ready",
            Self::Reconnecting => "reconnecting",
            Self::Unavailable => "unavailable",
            Self::Closing => "closing",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    /// The source has no token configured, or its configured variable is unset/empty.
    Unavailable(&'static str),
    /// The server rejected the session; the next operation re-initializes.
    SessionExpired,
    /// A protocol violation (version mismatch, missing capability, malformed result).
    Protocol(&'static str),
    /// The request was sent but no confirmed outcome was observed.
    Unknown(&'static str),
    /// Cancelled before the request was written; zero HTTP calls happened.
    CancelledBeforeSend,
    /// The local admission queue is full; refused before sending.
    Busy,
    /// A JSON-RPC error object addressed to this call.
    RpcError,
    /// The pool is shutting down.
    Closed,
}

impl CallError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable(code) => code,
            Self::SessionExpired => "session-expired",
            Self::Protocol(code) => code,
            Self::Unknown(_) => "remote-outcome-unknown",
            Self::CancelledBeforeSend => "cancelled",
            Self::Busy => "capacity",
            Self::RpcError => "remote-rpc-error",
            Self::Closed => "unavailable",
        }
    }
}

pub struct SourceSession {
    pub source_id: String,
    pub config_generation: i64,
    pub(super) transport: Arc<HttpTransport>,
    pub(super) state: Mutex<SessionState>,
    /// Consecutive initialize failures and the earliest time the next attempt may run. Implements
    /// the 1/2/4/8/16/30s source backoff.
    pub(super) failure_count: AtomicU32,
    pub(super) next_attempt_at: Mutex<Option<tokio::time::Instant>>,
    pub(super) source_permits: Arc<Semaphore>,
}

impl SourceSession {
    fn new(source_id: &str, config_generation: i64, transport: Arc<HttpTransport>) -> Self {
        Self {
            source_id: source_id.to_string(),
            config_generation,
            transport,
            state: Mutex::new(SessionState::Unavailable),
            failure_count: AtomicU32::new(0),
            next_attempt_at: Mutex::new(None),
            source_permits: Arc::new(Semaphore::new(MCP_CALLS_PER_SOURCE_MAX)),
        }
    }

    async fn backoff_until(&self) -> Option<tokio::time::Instant> {
        *self.next_attempt_at.lock().await
    }

    async fn record_success(&self) {
        self.failure_count.store(0, Ordering::SeqCst);
        *self.next_attempt_at.lock().await = None;
    }

    async fn record_failure(&self) {
        let count = self.failure_count.fetch_add(1, Ordering::SeqCst) as usize;
        let index = count.min(MCP_BACKOFF_SECONDS.len() - 1);
        let delay = Duration::from_secs(MCP_BACKOFF_SECONDS[index]);
        *self.next_attempt_at.lock().await = Some(tokio::time::Instant::now() + delay);
    }

    pub async fn state(&self) -> SessionState {
        *self.state.lock().await
    }

    /// Opens the optional GET SSE stream for this source, if the server offers it.
    pub async fn open_event_stream(&self) -> Option<super::transport::EventReceiver> {
        self.transport.open_event_stream().await
    }

    /// Initializes the session if needed. Concurrent callers serialize on the state mutex, so a
    /// reconnect storm collapses into one `initialize`.
    pub async fn ensure_ready(&self, deadline: tokio::time::Instant) -> Result<(), CallError> {
        let mut state = self.state.lock().await;
        match *state {
            SessionState::Ready => return Ok(()),
            SessionState::Disabled | SessionState::Closing => return Err(CallError::Closed),
            _ => {}
        }
        if let Some(next) = self.backoff_until().await {
            if tokio::time::Instant::now() < next {
                return Err(CallError::Unavailable("source-backoff"));
            }
        }
        *state = SessionState::Initializing;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            *state = SessionState::Unavailable;
            return Err(CallError::Unavailable("initialize-timeout"));
        }
        match self.initialize(remaining.min(MCP_INITIALIZE_TIMEOUT)).await {
            Ok(()) => {
                *state = SessionState::Ready;
                self.record_success().await;
                Ok(())
            }
            Err(error) => {
                self.record_failure().await;
                *state = match error {
                    CallError::Unavailable(_) | CallError::Closed => SessionState::Unavailable,
                    _ => SessionState::Reconnecting,
                };
                Err(error)
            }
        }
    }

    async fn initialize(&self, timeout: Duration) -> Result<(), CallError> {
        let params = json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "saaa", "version": "0.1.0" },
        });
        let result = self
            .transport
            .request(
                &format!("{}-init", crate::new_id("mcp")),
                "initialize",
                params,
                timeout,
            )
            .await
            .map_err(map_transport)?;
        let version = result.get("protocolVersion").and_then(Value::as_str);
        if version != Some(MCP_PROTOCOL_VERSION) {
            return Err(CallError::Protocol("protocol-version-mismatch"));
        }
        let tools_capable = result
            .get("capabilities")
            .and_then(|capabilities| capabilities.get("tools"))
            .is_some();
        if !tools_capable {
            return Err(CallError::Protocol("tools-capability-missing"));
        }
        // The initialized notification must complete before any list/call.
        self.transport
            .notify(
                "notifications/initialized",
                json!({}),
                timeout.min(MCP_CONNECT_TIMEOUT),
            )
            .await
            .map_err(map_transport)?;
        Ok(())
    }

    /// Sends `tools/call`. Admission is refused before any HTTP write when queues are full or the
    /// caller is already cancelled.
    pub async fn call(
        &self,
        tool_name: &str,
        arguments: Value,
        timeout: Duration,
        cancellation: &RunCancellation,
    ) -> Result<Value, CallError> {
        if cancellation.is_cancelled() {
            return Err(CallError::CancelledBeforeSend);
        }
        let deadline = tokio::time::Instant::now() + timeout;
        let _source_permit = self
            .source_permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| CallError::Busy)?;
        if cancellation.is_cancelled() {
            return Err(CallError::CancelledBeforeSend);
        }
        self.ensure_ready(deadline).await?;
        if cancellation.is_cancelled() {
            return Err(CallError::CancelledBeforeSend);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(CallError::Unknown("call-timeout"));
        }
        let call_id = crate::new_id("mcp-call");
        let future = self.transport.request(
            &call_id,
            "tools/call",
            json!({ "name": tool_name, "arguments": arguments }),
            remaining,
        );
        tokio::pin!(future);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                // Best effort only: the remote may still complete the effect.
                let _ = self
                    .transport
                    .notify(
                        "notifications/cancelled",
                        json!({ "requestId": call_id, "reason": "cancelled" }),
                        MCP_CONNECT_TIMEOUT,
                    )
                    .await;
                Err(CallError::Unknown("cancelled-after-send"))
            }
            result = &mut future => match result {
                Ok(value) => Ok(value),
                Err(error) => {
                    let mapped = map_transport(error);
                    if matches!(mapped, CallError::SessionExpired) {
                        let mut state = self.state.lock().await;
                        *state = SessionState::Reconnecting;
                    }
                    Err(mapped)
                }
            },
        }
    }

    /// Lists one page of tools. Uses the same state machine as `call` and the same deadline.
    pub async fn list_page(
        &self,
        cursor: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, CallError> {
        let deadline = tokio::time::Instant::now() + timeout;
        self.ensure_ready(deadline).await?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(CallError::Unknown("list-timeout"));
        }
        let params = match cursor {
            Some(cursor) => json!({ "cursor": cursor }),
            None => json!({}),
        };
        let id = crate::new_id("mcp-list");
        let result = self
            .transport
            .request(&id, "tools/list", params, remaining)
            .await
            .map_err(map_transport);
        if matches!(result, Err(CallError::SessionExpired)) {
            let mut state = self.state.lock().await;
            *state = SessionState::Reconnecting;
        }
        result
    }

    pub async fn shutdown(&self) {
        {
            let mut state = self.state.lock().await;
            *state = SessionState::Closing;
        }
        let _ = self.transport.delete_session().await;
        self.transport.close();
    }
}

fn map_transport(error: TransportError) -> CallError {
    match error {
        TransportError::Timeout => CallError::Unknown("remote-timeout"),
        TransportError::Connect => CallError::Unknown("remote-connect"),
        TransportError::SessionExpired => CallError::SessionExpired,
        TransportError::Protocol(code) => CallError::Protocol(code),
        TransportError::BodyTooLarge => CallError::Protocol("remote-too-large"),
        TransportError::Rpc { .. } => CallError::RpcError,
        // A 401/403 is a definite, non-retryable refusal and is kept distinct from an
        // indeterminate outcome.
        TransportError::Http(401 | 403) => CallError::Unavailable("remote-unauthorized"),
        TransportError::Http(_) => CallError::Unknown("remote-http"),
        TransportError::Closed => CallError::Closed,
    }
}

/// Profile-wide session registry. It enforces the per-source and per-profile concurrency caps and
/// owns the single-flight reconnect behavior through `SourceSession`.
pub struct McpSessionPool {
    pub(super) profile_permits: Arc<Semaphore>,
    pub(super) queue_permits: Arc<Semaphore>,
    pub(super) sessions: Mutex<HashMap<String, Arc<SourceSession>>>,
    pub(super) closing: AtomicBool,
}

impl Default for McpSessionPool {
    fn default() -> Self {
        Self::new()
    }
}

impl McpSessionPool {
    pub fn new() -> Self {
        Self {
            profile_permits: Arc::new(Semaphore::new(MCP_CALLS_PER_PROFILE_MAX)),
            queue_permits: Arc::new(Semaphore::new(MCP_CALL_QUEUE_MAX)),
            sessions: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    /// Resolves the session for a source and performs one admitted `tools/call`. The profile and
    /// source concurrency caps are enforced before any HTTP write.
    #[allow(clippy::too_many_arguments)]
    pub async fn call(
        &self,
        spec: &McpSourceSpec,
        config_generation: i64,
        tool_name: &str,
        arguments: Value,
        timeout: Duration,
        cancellation: &RunCancellation,
    ) -> Result<Value, CallError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let _admission = self.admit(deadline).await?;
        let session = self.session_for(spec, config_generation).await?;
        session
            .call(tool_name, arguments, timeout, cancellation)
            .await
    }

    /// Resolves (creating or replacing on generation change) the session for one source. A source
    /// whose token is configured but missing is reported unavailable without opening a connection.
    pub async fn session_for(
        &self,
        spec: &McpSourceSpec,
        config_generation: i64,
    ) -> Result<Arc<SourceSession>, CallError> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(CallError::Closed);
        }
        let token = spec.resolve_token();
        if spec.token_configured() && token.is_none() {
            return Err(CallError::Unavailable("missing-token"));
        }
        let mut sessions = self.sessions.lock().await;
        if let Some(existing) = sessions.get(&spec.id) {
            if existing.config_generation == config_generation {
                return Ok(existing.clone());
            }
            let old = existing.clone();
            tokio::spawn(async move { old.shutdown().await });
        }
        let transport = Arc::new(
            HttpTransport::new(&spec.url, token)
                .map_err(|_| CallError::Unavailable("transport"))?,
        );
        let session = Arc::new(SourceSession::new(&spec.id, config_generation, transport));
        sessions.insert(spec.id.clone(), session.clone());
        Ok(session)
    }

    /// Admission guard that must be held for the duration of one call. Refuses before send when
    /// the local queue is full (64 concurrent admissions) and waits for a profile call slot (16)
    /// until the call deadline. The connection/admission wait is part of the call deadline.
    pub async fn admit(&self, deadline: tokio::time::Instant) -> Result<AdmissionGuard, CallError> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(CallError::Closed);
        }
        let queue = self
            .queue_permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| CallError::Busy)?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(CallError::Busy);
        }
        let call = tokio::time::timeout(remaining, self.profile_permits.clone().acquire_owned())
            .await
            .map_err(|_| CallError::Busy)?
            .map_err(|_| CallError::Closed)?;
        Ok(AdmissionGuard {
            _queue: queue,
            _call: call,
        })
    }

    /// Stops new work, waits up to the shutdown grace for in-flight calls, then closes every
    /// session. In-flight calls that do not finish keep their indeterminate outcome.
    pub async fn shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
        let permits = MCP_CALLS_PER_PROFILE_MAX as u32;
        let drained = tokio::time::timeout(MCP_SHUTDOWN_GRACE, async {
            let mut held = Vec::new();
            for _ in 0..permits {
                match self.profile_permits.clone().acquire_owned().await {
                    Ok(permit) => held.push(permit),
                    Err(_) => break,
                }
            }
        })
        .await;
        let _ = drained;
        let sessions: Vec<Arc<SourceSession>> = {
            let mut guard = self.sessions.lock().await;
            guard.drain().map(|(_, session)| session).collect()
        };
        for session in sessions {
            session.shutdown().await;
        }
    }

    /// Closes and forgets the sessions of removed sources.
    pub async fn forget_sources(&self, keep: &std::collections::HashSet<String>) {
        let removed: Vec<Arc<SourceSession>> = {
            let mut guard = self.sessions.lock().await;
            let ids: Vec<String> = guard
                .keys()
                .filter(|id| !keep.contains(*id))
                .cloned()
                .collect();
            ids.into_iter().filter_map(|id| guard.remove(&id)).collect()
        };
        for session in removed {
            tokio::spawn(async move { session.shutdown().await });
        }
    }
}

/// Held for the duration of one admitted call; released on drop.
pub struct AdmissionGuard {
    pub(super) _queue: OwnedSemaphorePermit,
    pub(super) _call: OwnedSemaphorePermit,
}
