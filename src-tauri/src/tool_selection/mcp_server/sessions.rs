//! MCP session state and limits.
//!
//! A session owns exactly one host-generated conversation and run id. References, cached scenarios
//! and stored continuation results are scoped to that run, so nothing can be reused across
//! sessions. The registry enforces the 16-session cap, the idle TTL and the in-flight call caps.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::protocol::TypedRequestId;
use crate::tool_selection::contracts::now_ms;
use crate::RunCancellation;

pub const SESSION_MAX: usize = 16;
pub const SESSION_IDLE_TTL_MILLIS: i64 = 30 * 60 * 1000;
pub const SESSION_INITIALIZE_TIMEOUT_MILLIS: i64 = 10 * 1000;
pub const SESSION_CALLS_MAX: usize = 4;
pub const SESSION_GLOBAL_CALLS_MAX: usize = 16;
pub const SESSION_ID_HISTORY_MAX: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Initializing,
    Ready,
    Closing,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initializing => "initializing",
            Self::Ready => "ready",
            Self::Closing => "closing",
        }
    }
}

struct SessionInner {
    state: SessionState,
    last_activity_ms: i64,
    used_ids: HashSet<TypedRequestId>,
    inflight: HashMap<TypedRequestId, RunCancellation>,
}

pub struct Session {
    id: String,
    protocol_version: String,
    client_info: Option<Value>,
    conversation_id: String,
    run_id: String,
    principal_id: String,
    project_id: Option<String>,
    created_at_ms: i64,
    closing: AtomicBool,
    ready: AtomicBool,
    inner: Mutex<SessionInner>,
}

impl Session {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: String,
        protocol_version: String,
        client_info: Option<Value>,
        conversation_id: String,
        run_id: String,
        principal_id: String,
        project_id: Option<String>,
    ) -> Self {
        let now = now_ms();
        Self {
            id,
            protocol_version,
            client_info,
            conversation_id,
            run_id,
            principal_id,
            project_id,
            created_at_ms: now,
            closing: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            inner: Mutex::new(SessionInner {
                state: SessionState::Initializing,
                last_activity_ms: now,
                used_ids: HashSet::new(),
                inflight: HashMap::new(),
            }),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }
    pub fn client_info(&self) -> Option<&Value> {
        self.client_info.as_ref()
    }
    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }
    pub fn project_id(&self) -> Option<&str> {
        self.project_id.as_deref()
    }

    pub fn state(&self) -> SessionState {
        self.inner
            .lock()
            .map(|inner| inner.state)
            .unwrap_or(SessionState::Closing)
    }

    pub fn mark_ready(&self) {
        self.ready.store(true, Ordering::SeqCst);
        if let Ok(mut inner) = self.inner.lock() {
            if inner.state == SessionState::Initializing {
                inner.state = SessionState::Ready;
            }
            inner.last_activity_ms = now_ms();
        }
    }

    /// False until `notifications/initialized` is accepted. A never-ready session owns an empty
    /// conversation that can be safely removed when the session is discarded.
    pub fn was_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    pub fn mark_closing(&self) {
        self.closing.store(true, Ordering::SeqCst);
        if let Ok(mut inner) = self.inner.lock() {
            inner.state = SessionState::Closing;
        }
    }

    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::SeqCst)
    }

    pub fn touch(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.last_activity_ms = now_ms();
        }
    }

    pub fn in_flight_count(&self) -> usize {
        self.inner
            .lock()
            .map(|inner| inner.inflight.len())
            .unwrap_or(0)
    }

    /// Cancels every in-flight call in this session. Used by DELETE and shutdown.
    pub fn cancel_all(&self) {
        let cancellations: Vec<RunCancellation> = self
            .inner
            .lock()
            .map(|inner| inner.inflight.values().cloned().collect())
            .unwrap_or_default();
        for cancellation in cancellations {
            cancellation.cancel();
        }
    }

    /// Cancels one typed request id. A finished or unknown id is a no-op.
    pub fn cancel(&self, id: &TypedRequestId) -> bool {
        let cancellation = self
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.inflight.get(id).cloned());
        match cancellation {
            Some(cancellation) => {
                cancellation.cancel();
                true
            }
            None => false,
        }
    }

    fn finish(&self, id: &TypedRequestId) -> bool {
        self.inner
            .lock()
            .map(|mut inner| {
                if inner.inflight.remove(id).is_some() {
                    inner.last_activity_ms = now_ms();
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false)
    }

    /// An uninitialized session expires 10 seconds after `initialize`, a hard deadline that
    /// `ping`/`tools/list` cannot extend; a client that keeps pinging still loses the slot.
    fn initializing_expired(&self, now: i64) -> bool {
        self.inner
            .lock()
            .map(|inner| {
                inner.state == SessionState::Initializing
                    && now - self.created_at_ms > SESSION_INITIALIZE_TIMEOUT_MILLIS
            })
            .unwrap_or(false)
    }

    fn idle_expired(&self, now: i64) -> bool {
        self.inner
            .lock()
            .map(|inner| {
                inner.inflight.is_empty()
                    && inner.state != SessionState::Closing
                    && now - inner.last_activity_ms > SESSION_IDLE_TTL_MILLIS
            })
            .unwrap_or(false)
    }

    /// Test-only: ages a session so the idle TTL can be exercised without a real 30-minute wait.
    #[cfg(test)]
    pub(crate) fn force_last_activity(&self, at_ms: i64) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.last_activity_ms = at_ms;
        }
    }

    /// Test-only: backdates creation so the initialization deadline can be exercised.
    #[cfg(test)]
    fn force_created_at(&mut self, at_ms: i64) {
        self.created_at_ms = at_ms;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReserveResult {
    Reserved,
    Duplicate,
    HistoryFull,
    SessionLimit,
    GlobalLimit,
    Closing,
}

pub struct SessionRegistry {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    global_inflight: AtomicUsize,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            global_inflight: AtomicUsize::new(0),
        }
    }

    pub fn len(&self) -> usize {
        self.sessions.lock().map(|map| map.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn global_in_flight(&self) -> usize {
        self.global_inflight.load(Ordering::SeqCst)
    }

    /// Inserts a freshly initialized session. The caller purges expired sessions first, so the
    /// capacity check here is authoritative; a race returns `session-limit` and the caller must
    /// not leave a conversation behind.
    pub fn insert(&self, session: Session) -> Result<Arc<Session>, &'static str> {
        let mut sessions = self.sessions.lock().map_err(|_| "session-lock")?;
        if sessions.len() >= SESSION_MAX {
            return Err("session-limit");
        }
        let session = Arc::new(session);
        sessions.insert(session.id.clone(), session.clone());
        Ok(session)
    }

    /// Resolves a session id. An idle-expired session is treated as unknown; other sessions are
    /// not purged here so their scopes are released by the explicit purge path.
    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        let sessions = self.sessions.lock().ok()?;
        let session = sessions.get(id)?;
        if session.idle_expired(now_ms()) {
            return None;
        }
        Some(session.clone())
    }

    /// Removes a session and returns it for cleanup. `None` when the id is unknown.
    pub fn remove(&self, id: &str) -> Option<Arc<Session>> {
        let mut sessions = self.sessions.lock().ok()?;
        let session = sessions.remove(id);
        if let Some(session) = &session {
            session.mark_closing();
        }
        session
    }

    pub fn sessions(&self) -> Vec<Arc<Session>> {
        self.sessions
            .lock()
            .map(|map| map.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Drops idle and never-initialized sessions beyond their TTL. Returns the removed sessions so
    /// the caller can release their scopes.
    pub fn purge_expired(&self) -> Vec<Arc<Session>> {
        let now = now_ms();
        let mut sessions = match self.sessions.lock() {
            Ok(sessions) => sessions,
            Err(_) => return Vec::new(),
        };
        let mut removed = Vec::new();
        sessions.retain(|_, session| {
            let expired = session.initializing_expired(now) || session.idle_expired(now);
            if expired {
                session.mark_closing();
                removed.push(session.clone());
            }
            !expired
        });
        removed
    }

    /// Reserves one in-flight call slot for a typed request id and records the id in the history.
    /// The id history is never pruned: once it is full, new requests are refused rather than
    /// discarding the evidence needed to detect a replayed side effect.
    pub fn reserve_call(
        &self,
        session: &Session,
        id: &TypedRequestId,
        cancellation: RunCancellation,
    ) -> ReserveResult {
        let mut inner = match session.inner.lock() {
            Ok(inner) => inner,
            Err(_) => return ReserveResult::Closing,
        };
        if inner.state == SessionState::Closing {
            return ReserveResult::Closing;
        }
        if inner.used_ids.contains(id) {
            return ReserveResult::Duplicate;
        }
        if inner.used_ids.len() >= SESSION_ID_HISTORY_MAX {
            return ReserveResult::HistoryFull;
        }
        if inner.inflight.len() >= SESSION_CALLS_MAX {
            return ReserveResult::SessionLimit;
        }
        let mut current = self.global_inflight.load(Ordering::SeqCst);
        loop {
            if current >= SESSION_GLOBAL_CALLS_MAX {
                return ReserveResult::GlobalLimit;
            }
            match self.global_inflight.compare_exchange_weak(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        inner.used_ids.insert(id.clone());
        inner.inflight.insert(id.clone(), cancellation);
        inner.last_activity_ms = now_ms();
        ReserveResult::Reserved
    }

    /// Releases one reserved slot. Must be paired with every `Reserved` result; a no-op drop for an
    /// unknown or already finished id keeps the global counter from drifting.
    pub fn finish_call(&self, session: &Session, id: &TypedRequestId) {
        if session.finish(id) {
            self.global_inflight.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Cancels every call and marks every session closing. Returns the sessions so their scopes can
    /// be released.
    pub fn close_all(&self) -> Vec<Arc<Session>> {
        let sessions = self.sessions().into_iter().collect::<Vec<_>>();
        for session in &sessions {
            session.mark_closing();
            session.cancel_all();
        }
        sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str) -> Session {
        Session::new(
            id.to_string(),
            "2025-06-18".to_string(),
            None,
            format!("conv-{id}"),
            format!("run-{id}"),
            "P1".to_string(),
            None,
        )
    }

    #[test]
    fn duplicate_ids_are_rejected_and_typed_ids_stay_distinct() {
        let registry = SessionRegistry::new();
        let session = registry.insert(session("s1")).expect("insert");
        let first = TypedRequestId::Integer(1);
        assert_eq!(
            registry.reserve_call(&session, &first, RunCancellation::default()),
            ReserveResult::Reserved
        );
        assert_eq!(
            registry.reserve_call(&session, &first, RunCancellation::default()),
            ReserveResult::Duplicate
        );
        // `"1"` is a different request from `1`.
        assert_eq!(
            registry.reserve_call(
                &session,
                &TypedRequestId::String("1".to_string()),
                RunCancellation::default()
            ),
            ReserveResult::Reserved
        );
        assert_eq!(registry.global_in_flight(), 2);
        registry.finish_call(&session, &first);
        assert_eq!(registry.global_in_flight(), 1);
    }

    #[test]
    fn session_and_global_call_limits_are_enforced() {
        let registry = SessionRegistry::new();
        let session = registry.insert(session("s1")).expect("insert");
        for index in 0..SESSION_CALLS_MAX {
            assert_eq!(
                registry.reserve_call(
                    &session,
                    &TypedRequestId::Integer(index as i64),
                    RunCancellation::default()
                ),
                ReserveResult::Reserved
            );
        }
        assert_eq!(
            registry.reserve_call(
                &session,
                &TypedRequestId::Integer(99),
                RunCancellation::default()
            ),
            ReserveResult::SessionLimit
        );
    }

    #[test]
    fn global_call_limit_is_enforced_across_sessions() {
        let registry = SessionRegistry::new();
        let sessions: Vec<Arc<Session>> = (0..SESSION_GLOBAL_CALLS_MAX / SESSION_CALLS_MAX)
            .map(|index| {
                registry
                    .insert(session(&format!("s{index}")))
                    .expect("insert")
            })
            .collect();
        for (session_index, session) in sessions.iter().enumerate() {
            for call in 0..SESSION_CALLS_MAX {
                let id = TypedRequestId::Integer((session_index * SESSION_CALLS_MAX + call) as i64);
                assert_eq!(
                    registry.reserve_call(session, &id, RunCancellation::default()),
                    ReserveResult::Reserved
                );
            }
        }
        assert_eq!(registry.global_in_flight(), SESSION_GLOBAL_CALLS_MAX);
        let extra = registry.insert(session("extra")).expect("insert");
        assert_eq!(
            registry.reserve_call(
                &extra,
                &TypedRequestId::Integer(999),
                RunCancellation::default()
            ),
            ReserveResult::GlobalLimit
        );
        // A refused global reservation must not leak a per-session slot either.
        assert_eq!(extra.in_flight_count(), 0);
    }

    #[test]
    fn id_history_is_bounded_and_a_full_history_refuses_new_ids() {
        let registry = SessionRegistry::new();
        let session = registry.insert(session("ids")).expect("insert");
        for index in 0..SESSION_ID_HISTORY_MAX {
            let id = TypedRequestId::Integer(index as i64);
            assert_eq!(
                registry.reserve_call(&session, &id, RunCancellation::default()),
                ReserveResult::Reserved
            );
            registry.finish_call(&session, &id);
        }
        // Every slot is free, but the history is full: refuse rather than pruning evidence.
        assert_eq!(
            registry.reserve_call(
                &session,
                &TypedRequestId::Integer(-1),
                RunCancellation::default()
            ),
            ReserveResult::HistoryFull
        );
        assert_eq!(registry.global_in_flight(), 0);
    }

    #[test]
    fn session_cap_is_enforced() {
        let registry = SessionRegistry::new();
        for index in 0..SESSION_MAX {
            registry
                .insert(session(&format!("s{index}")))
                .expect("insert");
        }
        assert!(registry.insert(session("overflow")).is_err());
    }

    #[test]
    fn idle_sessions_are_purged_and_no_longer_resolve() {
        let registry = SessionRegistry::new();
        let session = registry.insert(session("idle")).expect("insert");
        session.force_last_activity(now_ms() - SESSION_IDLE_TTL_MILLIS - 1);
        assert!(registry.get("idle").is_none());
        let purged = registry.purge_expired();
        assert_eq!(purged.len(), 1);
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn initialization_deadline_is_not_extended_by_activity() {
        let registry = SessionRegistry::new();
        let mut session = session("init");
        session.force_created_at(now_ms() - SESSION_INITIALIZE_TIMEOUT_MILLIS - 1);
        // Any later request (ping/tools/list) touches the session; the hard deadline still wins.
        session.touch();
        let session = registry.insert(session).expect("insert");
        assert!(session.initializing_expired(now_ms()));
        assert_eq!(registry.purge_expired().len(), 1);
        assert_eq!(registry.len(), 0);
    }
}
