pub(crate) mod client;
pub(crate) mod contracts;
pub(crate) mod session;

use client::{
    AllocationStart, Cancellation, ChatCompletion, ChatMessage, CleanupResult, EphemeralCredential,
    LarmError, LarmHttpClient, OperationProgress, SharedLarmClient,
};
use contracts::{BoundedIdentifier, ReadyAllocation, ReleaseFailureKind, SessionFailureKind};
use session::{AllocationSession, SessionEffect, SessionPhase, SessionSignal};
use std::{ops::Deref, sync::Arc, time::Duration};

pub(crate) const CONTRACT_COMMIT: &str = "7dca7c3";

pub(crate) enum LarmRuntimeGate {
    Disabled,
    Ready(Arc<SharedLarmClient>),
    Unavailable,
}

impl LarmRuntimeGate {
    pub(crate) fn initialize() -> Self {
        if !feature_flag_enabled(std::env::var("SAAA_LARM_ENABLED").ok().as_deref()) {
            return Self::Disabled;
        }
        match SharedLarmClient::build() {
            Ok(client) => Self::Ready(Arc::new(client)),
            Err(_) => Self::Unavailable,
        }
    }

    pub(crate) fn state(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Ready(_) => "ready",
            Self::Unavailable => "unavailable",
        }
    }

    pub(crate) fn allows_traffic(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    pub(crate) fn public_message(&self) -> &'static str {
        match self {
            Self::Disabled => {
                "LARM is disabled. Set SAAA_LARM_ENABLED=1 and restart SAAA to enable it."
            }
            Self::Ready(_) => "LARM runtime support is enabled.",
            Self::Unavailable => {
                "LARM runtime support could not be initialized. Restart SAAA after checking the local runtime configuration."
            }
        }
    }

    fn client(&self) -> Result<&SharedLarmClient, SessionFailureKind> {
        match self {
            Self::Ready(client) => Ok(client),
            Self::Disabled | Self::Unavailable => Err(SessionFailureKind::Unavailable),
        }
    }
}

fn feature_flag_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

pub(crate) struct LarmProvider<'a> {
    shared: &'a SharedLarmClient,
    base_url: String,
    credential: EphemeralCredential,
    ttl_seconds: u32,
    startup_timeout_seconds: u32,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum AllocationCleanup {
    NotStarted,
    Released,
    DeferredToTtl(ReleaseFailureKind),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct AllocationFailure {
    pub(crate) kind: SessionFailureKind,
    pub(crate) cleanup: AllocationCleanup,
}

#[derive(Debug)]
pub(crate) struct ReadyLease {
    allocation: ReadyAllocation,
    received_at: tokio::time::Instant,
}

impl ReadyLease {
    fn received(allocation: ReadyAllocation) -> Self {
        Self {
            allocation,
            received_at: tokio::time::Instant::now(),
        }
    }
}

impl Deref for ReadyLease {
    type Target = ReadyAllocation;

    fn deref(&self) -> &Self::Target {
        &self.allocation
    }
}

impl<'a> LarmProvider<'a> {
    pub(crate) fn for_attempt(
        gate: &'a LarmRuntimeGate,
        base_url: &str,
        ttl_seconds: u32,
        startup_timeout_seconds: u32,
    ) -> Result<Self, SessionFailureKind> {
        let shared = gate.client()?;
        let credential = EphemeralCredential::from_environment()?;
        LarmHttpClient::new(shared, base_url, &credential, ttl_seconds)?;
        if !(1..=300).contains(&startup_timeout_seconds) {
            return Err(SessionFailureKind::Contract);
        }
        Ok(Self {
            shared,
            base_url: base_url.to_string(),
            credential,
            ttl_seconds,
            startup_timeout_seconds,
        })
    }

    pub(crate) async fn probe(
        gate: &'a LarmRuntimeGate,
        base_url: &str,
    ) -> Result<(), SessionFailureKind> {
        let client = gate.client()?;
        LarmHttpClient::new_probe(client, base_url)?.probe().await
    }

    pub(crate) async fn allocate_ready(
        &self,
        cancellation: Cancellation<'_>,
    ) -> Result<ReadyLease, AllocationFailure> {
        let started = tokio::time::Instant::now();
        let first = self.allocate_ready_once(cancellation, started).await;
        if first.as_ref().is_err_and(|failure| {
            failure.kind.permits_pre_stream_reallocation()
                && elapsed_ms(started)
                    < u64::from(self.startup_timeout_seconds).saturating_mul(1_000)
        }) {
            return self.allocate_ready_once(cancellation, started).await;
        }
        first
    }

    async fn allocate_ready_once(
        &self,
        cancellation: Cancellation<'_>,
        started: tokio::time::Instant,
    ) -> Result<ReadyLease, AllocationFailure> {
        let http = self.http().map_err(|kind| AllocationFailure {
            kind,
            cleanup: AllocationCleanup::NotStarted,
        })?;
        let startup_deadline_ms = u64::from(self.startup_timeout_seconds).saturating_mul(1_000);
        let before_allocate_ms = elapsed_ms(started);
        if before_allocate_ms >= startup_deadline_ms {
            return Err(AllocationFailure {
                kind: SessionFailureKind::Timeout,
                cleanup: AllocationCleanup::NotStarted,
            });
        }
        let mut session =
            AllocationSession::new(0, self.startup_timeout_seconds).map_err(|_| {
                AllocationFailure {
                    kind: SessionFailureKind::Internal,
                    cleanup: AllocationCleanup::NotStarted,
                }
            })?;
        session
            .transition(SessionSignal::Start)
            .map_err(|_| AllocationFailure {
                kind: SessionFailureKind::Internal,
                cleanup: AllocationCleanup::NotStarted,
            })?;

        if cancellation.is_cancelled() {
            return Err(AllocationFailure {
                kind: SessionFailureKind::Cancelled,
                cleanup: AllocationCleanup::NotStarted,
            });
        }

        let start = match tokio::select! {
            _ = cancellation.cancelled() => Err(LarmError::new(SessionFailureKind::Cancelled, false)),
            _ = tokio::time::sleep(Duration::from_millis(startup_deadline_ms - before_allocate_ms)) => {
                Err(LarmError::new(SessionFailureKind::AllocationOutcomeUnknown, false))
            }
            result = http.allocate(cancellation) => result,
        } {
            Ok(start) => start,
            Err(error) => {
                return Err(AllocationFailure {
                    kind: error.kind,
                    cleanup: match error.kind {
                        SessionFailureKind::AllocationOutcomeUnknown => {
                            AllocationCleanup::DeferredToTtl(ReleaseFailureKind::Network)
                        }
                        SessionFailureKind::Cancelled => {
                            AllocationCleanup::DeferredToTtl(ReleaseFailureKind::Internal)
                        }
                        _ => AllocationCleanup::NotStarted,
                    },
                })
            }
        };
        match start {
            AllocationStart::Ready(allocation) => {
                let effects = session
                    .transition(SessionSignal::AllocateReady {
                        now_ms: elapsed_ms(started),
                        allocation: allocation.clone(),
                    })
                    .map_err(|_| AllocationFailure {
                        kind: SessionFailureKind::Protocol,
                        cleanup: AllocationCleanup::NotStarted,
                    })?;
                if session.phase() == SessionPhase::Ready && effects.is_empty() {
                    Ok(ReadyLease::received(allocation))
                } else {
                    Err(self
                        .cleanup_failure(
                            &http,
                            Some(&allocation.allocation_id),
                            SessionFailureKind::Timeout,
                        )
                        .await)
                }
            }
            AllocationStart::Pending(pending) => {
                let mut poll_index = 0_u64;
                let effects = session
                    .transition(SessionSignal::AllocatePending {
                        now_ms: elapsed_ms(started),
                        pending: pending.clone(),
                        jitter_percent: poll_jitter_percent(
                            pending.operation_id.as_str(),
                            poll_index,
                        ),
                    })
                    .map_err(|_| AllocationFailure {
                        kind: SessionFailureKind::Protocol,
                        cleanup: AllocationCleanup::NotStarted,
                    })?;
                let mut effect = effects.into_iter().next();
                loop {
                    let Some(SessionEffect::SchedulePoll {
                        after_ms,
                        deadline_ms,
                        ..
                    }) = effect
                    else {
                        return Err(self
                            .cleanup_failure(
                                &http,
                                pending.cleanup_allocation_id.as_ref(),
                                SessionFailureKind::Protocol,
                            )
                            .await);
                    };
                    if elapsed_ms(started) >= deadline_ms {
                        return Err(self
                            .cleanup_failure(
                                &http,
                                pending.cleanup_allocation_id.as_ref(),
                                SessionFailureKind::Timeout,
                            )
                            .await);
                    }
                    tokio::select! {
                        _ = cancellation.cancelled() => {
                            return Err(self.cleanup_failure(
                                &http,
                                pending.cleanup_allocation_id.as_ref(),
                                SessionFailureKind::Cancelled,
                            ).await)
                        },
                        _ = tokio::time::sleep(Duration::from_millis(after_ms)) => {}
                    }
                    let now_ms = elapsed_ms(started);
                    if now_ms >= deadline_ms {
                        return Err(self
                            .cleanup_failure(
                                &http,
                                pending.cleanup_allocation_id.as_ref(),
                                SessionFailureKind::Timeout,
                            )
                            .await);
                    }
                    let operation = tokio::select! {
                        _ = cancellation.cancelled() => Err(LarmError::new(SessionFailureKind::Cancelled, false)),
                        _ = tokio::time::sleep(Duration::from_millis(deadline_ms - now_ms)) => {
                            Err(LarmError::new(SessionFailureKind::Timeout, false))
                        }
                        result = http.get_operation(
                            &pending.operation_id,
                            pending.cleanup_allocation_id.as_ref(),
                            cancellation,
                        ) => result,
                    };
                    match operation {
                        Ok(OperationProgress::Pending) => {
                            poll_index = poll_index.saturating_add(1);
                            effect = session
                                .transition(SessionSignal::PollPending {
                                    now_ms: elapsed_ms(started),
                                    pending: pending.clone(),
                                    jitter_percent: poll_jitter_percent(
                                        pending.operation_id.as_str(),
                                        poll_index,
                                    ),
                                })
                                .ok()
                                .and_then(|effects| effects.into_iter().next());
                        }
                        Ok(OperationProgress::Succeeded) => {
                            let Some(allocation_id) = pending.cleanup_allocation_id.as_ref() else {
                                return Err(AllocationFailure {
                                    kind: SessionFailureKind::Protocol,
                                    cleanup: AllocationCleanup::NotStarted,
                                });
                            };
                            let now_ms = elapsed_ms(started);
                            if now_ms >= deadline_ms {
                                return Err(self
                                    .cleanup_failure(
                                        &http,
                                        Some(allocation_id),
                                        SessionFailureKind::Timeout,
                                    )
                                    .await);
                            }
                            let ready = tokio::select! {
                                _ = cancellation.cancelled() => Err(LarmError::new(SessionFailureKind::Cancelled, false)),
                                _ = tokio::time::sleep(Duration::from_millis(deadline_ms - now_ms)) => {
                                    Err(LarmError::new(SessionFailureKind::Timeout, false))
                                }
                                result = http.get_allocation(allocation_id, cancellation) => result,
                            };
                            match ready {
                                Ok(allocation) => {
                                    let effects = session
                                        .transition(SessionSignal::PollReady {
                                            now_ms: elapsed_ms(started),
                                            allocation: allocation.clone(),
                                        })
                                        .map_err(|_| AllocationFailure {
                                            kind: SessionFailureKind::Protocol,
                                            cleanup: AllocationCleanup::NotStarted,
                                        })?;
                                    if session.phase() == SessionPhase::Ready && effects.is_empty()
                                    {
                                        return Ok(ReadyLease::received(allocation));
                                    }
                                    return Err(self
                                        .cleanup_failure(
                                            &http,
                                            Some(allocation_id),
                                            SessionFailureKind::Timeout,
                                        )
                                        .await);
                                }
                                Err(error) => {
                                    return Err(self
                                        .cleanup_failure(&http, Some(allocation_id), error.kind)
                                        .await)
                                }
                            }
                        }
                        Err(error) => {
                            return Err(self
                                .cleanup_failure(
                                    &http,
                                    pending.cleanup_allocation_id.as_ref(),
                                    error.kind,
                                )
                                .await)
                        }
                    }
                }
            }
        }
    }

    pub(crate) async fn chat<F>(
        &self,
        lease: &ReadyLease,
        messages: &[ChatMessage],
        timeout: Duration,
        cancellation: Cancellation<'_>,
        on_delta: F,
    ) -> Result<ChatCompletion, LarmError>
    where
        F: FnMut(&str, bool) -> Result<(), SessionFailureKind>,
    {
        self.http()
            .map_err(|kind| LarmError::new(kind, false))?
            .chat(
                &lease.allocation,
                lease.received_at,
                messages,
                timeout,
                cancellation,
                on_delta,
            )
            .await
    }

    pub(crate) async fn release(&self, allocation_id: &BoundedIdentifier) -> CleanupResult {
        match self.http() {
            Ok(http) => http.release(allocation_id).await,
            Err(kind) => CleanupResult::DeferredToTtl(release_kind(kind)),
        }
    }

    fn http(&self) -> Result<LarmHttpClient<'_>, SessionFailureKind> {
        LarmHttpClient::new(
            self.shared,
            &self.base_url,
            &self.credential,
            self.ttl_seconds,
        )
    }

    async fn cleanup_failure(
        &self,
        http: &LarmHttpClient<'_>,
        allocation_id: Option<&BoundedIdentifier>,
        kind: SessionFailureKind,
    ) -> AllocationFailure {
        let cleanup = match allocation_id {
            Some(allocation_id) => match http.release(allocation_id).await {
                CleanupResult::Released => AllocationCleanup::Released,
                CleanupResult::DeferredToTtl(kind) => AllocationCleanup::DeferredToTtl(kind),
            },
            None => AllocationCleanup::NotStarted,
        };
        AllocationFailure { kind, cleanup }
    }
}

fn elapsed_ms(started: tokio::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn poll_jitter_percent(operation_id: &str, poll_index: u64) -> i8 {
    let mut hash = 0xcbf29ce484222325_u64 ^ poll_index;
    for byte in operation_id.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    i8::try_from(hash % 41).unwrap_or(20) - 20
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::AtomicBool,
        thread,
    };

    #[test]
    fn gate_public_messages_are_bounded() {
        for gate in [LarmRuntimeGate::Disabled, LarmRuntimeGate::Unavailable] {
            assert!(gate.public_message().len() <= 300);
        }
    }

    #[test]
    fn feature_flag_accepts_only_the_exact_one_literal() {
        assert!(feature_flag_enabled(Some("1")));
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("true"),
            Some("01"),
            Some("1 "),
        ] {
            assert!(!feature_flag_enabled(value));
        }
    }

    #[test]
    fn production_poll_jitter_stays_within_twenty_percent() {
        let values = (0..100)
            .map(|index| poll_jitter_percent("operation_1", index))
            .collect::<Vec<_>>();
        assert!(values.iter().all(|value| (-20..=20).contains(value)));
        assert!(values.iter().any(|value| *value != 0));
    }

    #[test]
    fn contract_and_session_modules_have_no_io_dependencies() {
        for source in [include_str!("contracts.rs"), include_str!("session.rs")] {
            for forbidden in ["reqwest::", "rusqlite::", "tauri::", "std::fs", "std::net"] {
                assert!(
                    !source.contains(forbidden),
                    "pure LARM module contains forbidden dependency: {forbidden}"
                );
            }
        }
    }

    #[tokio::test]
    async fn disabled_gate_blocks_connection_test_before_network_access() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("sentinel binds");
        listener
            .set_nonblocking(true)
            .expect("sentinel becomes nonblocking");
        let address = listener.local_addr().expect("sentinel address");
        assert_eq!(
            LarmProvider::probe(&LarmRuntimeGate::Disabled, &format!("http://{address}/")).await,
            Err(SessionFailureKind::Unavailable)
        );
        assert!(
            listener.accept().is_err(),
            "disabled gate touched the network"
        );
    }

    #[tokio::test]
    async fn daemon_restart_reallocates_once_before_gateway_use() {
        fn response(status: &str, body: &str) -> String {
            format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
        }
        fn allocation(id: &str, status: &str, operation: &str) -> String {
            format!(
                r#"{{"id":"{id}","status":"{status}","requirements":[{{"capability":"llm.general","route":"llm-default"}}],"bindings":[{{"capability":"llm.general","route":"llm-default","runtime":"runtime_1","node":"gnosis","status":"HOT","candidateRank":1,"fallback":false,"selectionReason":"primary-live"}}],"allowFallback":false,"deploymentPolicy":"existing-only","createdAt":"2026-08-28T00:00:00.000Z","expiresAt":"2026-08-28T00:05:00.000Z"{operation}}}"#
            )
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("fake LARM binds");
        let address = listener.local_addr().expect("fake LARM address");
        let pending = allocation("alloc_old", "pending", r#","operationId":"op_old""#);
        let ready = allocation("alloc_new", "ready", "");
        let not_found = r#"{"error":{"code":"not_found","message":"gone"}}"#;
        let server = thread::spawn(move || {
            for response in [
                response("202 Accepted", &pending),
                response("404 Not Found", not_found),
                response("404 Not Found", not_found),
                response("200 OK", &ready),
            ] {
                let (mut socket, _) = listener.accept().expect("fake LARM accepts");
                let mut request = [0_u8; 16 * 1_024];
                let _ = socket.read(&mut request).expect("fake LARM reads");
                socket
                    .write_all(response.as_bytes())
                    .expect("fake LARM writes");
            }
        });
        let shared = SharedLarmClient::build().expect("client builds");
        let provider = LarmProvider {
            shared: &shared,
            base_url: format!("http://{address}/"),
            credential: EphemeralCredential::fixture(),
            ttl_seconds: 300,
            startup_timeout_seconds: 5,
        };
        let flag = AtomicBool::new(false);
        let notify = tokio::sync::Notify::new();
        let ready = provider
            .allocate_ready(Cancellation {
                flag: &flag,
                notify: &notify,
            })
            .await
            .expect("replacement allocation becomes ready");
        server.join().expect("fake LARM joins");
        assert_eq!(ready.allocation_id.as_str(), "alloc_new");
    }
}
