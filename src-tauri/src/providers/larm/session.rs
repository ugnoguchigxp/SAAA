use super::contracts::{
    BoundedIdentifier, ContractError, PendingAllocation, ReadyAllocation, ReleaseFailureKind,
    SessionFailureKind,
};

const MAX_JITTER_PERCENT: i8 = 20;
const STREAM_RENEW_MAX_ATTEMPTS: u8 = 2;
const STREAM_RENEW_RETRY_DELAY_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionPhase {
    Idle,
    Allocating,
    Pending,
    Ready,
    Renewing,
    Releasing,
    Released,
    Expired,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalOutcome {
    Completed,
    Cancelled,
    Failed(SessionFailureKind),
    Expired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionCleanupOutcome {
    NotStarted,
    Pending,
    Released,
    DeferredToTtl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SessionEffect {
    Allocate,
    SchedulePoll {
        operation_id: BoundedIdentifier,
        after_ms: u64,
        deadline_ms: u64,
    },
    Renew {
        allocation_id: BoundedIdentifier,
    },
    BeginStream {
        allocation_id: BoundedIdentifier,
        binding_fingerprint: BoundedIdentifier,
    },
    Release {
        allocation_id: BoundedIdentifier,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SessionSignal {
    Start,
    AllocateReady {
        now_ms: u64,
        allocation: ReadyAllocation,
    },
    AllocatePending {
        now_ms: u64,
        pending: PendingAllocation,
        jitter_percent: i8,
    },
    PollPending {
        now_ms: u64,
        pending: PendingAllocation,
        jitter_percent: i8,
    },
    PollReady {
        now_ms: u64,
        allocation: ReadyAllocation,
    },
    Tick {
        now_ms: u64,
    },
    RenewReady {
        now_ms: u64,
        allocation: ReadyAllocation,
    },
    RenewFailed {
        now_ms: u64,
        kind: SessionFailureKind,
    },
    StreamStarted {
        now_ms: u64,
    },
    OutputStarted,
    Completed,
    Cancelled,
    Failed(SessionFailureKind),
    AllocateOutcomeUnknown,
    DaemonRestarted {
        now_ms: u64,
    },
    ReleaseSucceeded,
    ReleaseFailed(ReleaseFailureKind),
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct AllocationSession {
    phase: SessionPhase,
    startup_deadline_ms: u64,
    pending: Option<PendingAllocation>,
    allocation: Option<ReadyAllocation>,
    poll_schedule_index: u8,
    renew_at_ms: Option<u64>,
    renew_retry_at_ms: Option<u64>,
    expires_at_ms: Option<u64>,
    stream_started: bool,
    output_started: bool,
    reallocated: bool,
    renew_attempts: u8,
    release_requested: bool,
    release_finished: bool,
    terminal_outcome: Option<TerminalOutcome>,
    cleanup_outcome: SessionCleanupOutcome,
    release_failure_kind: Option<ReleaseFailureKind>,
}

impl AllocationSession {
    pub(crate) fn new(now_ms: u64, startup_timeout_seconds: u32) -> Result<Self, SessionError> {
        if !(1..=300).contains(&startup_timeout_seconds) {
            return Err(SessionError::InvalidStartupTimeout);
        }
        let startup_deadline_ms = now_ms
            .checked_add(u64::from(startup_timeout_seconds) * 1_000)
            .ok_or(SessionError::TimeOverflow)?;
        Ok(Self {
            phase: SessionPhase::Idle,
            startup_deadline_ms,
            pending: None,
            allocation: None,
            poll_schedule_index: 0,
            renew_at_ms: None,
            renew_retry_at_ms: None,
            expires_at_ms: None,
            stream_started: false,
            output_started: false,
            reallocated: false,
            renew_attempts: 0,
            release_requested: false,
            release_finished: false,
            terminal_outcome: None,
            cleanup_outcome: SessionCleanupOutcome::NotStarted,
            release_failure_kind: None,
        })
    }

    pub(crate) fn phase(&self) -> SessionPhase {
        self.phase
    }

    pub(crate) fn startup_deadline_ms(&self) -> u64 {
        self.startup_deadline_ms
    }

    pub(crate) fn renew_at_ms(&self) -> Option<u64> {
        self.renew_retry_at_ms.or(self.renew_at_ms)
    }

    pub(crate) fn expires_at_ms(&self) -> Option<u64> {
        self.expires_at_ms
    }

    pub(crate) fn allocation(&self) -> Option<&ReadyAllocation> {
        self.allocation.as_ref()
    }

    pub(crate) fn stream_started(&self) -> bool {
        self.stream_started
    }

    pub(crate) fn output_started(&self) -> bool {
        self.output_started
    }

    pub(crate) fn terminal_outcome(&self) -> Option<TerminalOutcome> {
        self.terminal_outcome
    }

    pub(crate) fn cleanup_outcome(&self) -> SessionCleanupOutcome {
        self.cleanup_outcome
    }

    pub(crate) fn release_failure_kind(&self) -> Option<ReleaseFailureKind> {
        self.release_failure_kind
    }

    pub(crate) fn transition(
        &mut self,
        signal: SessionSignal,
    ) -> Result<Vec<SessionEffect>, SessionError> {
        match signal {
            SessionSignal::Start => {
                self.require_phase(SessionPhase::Idle)?;
                self.phase = SessionPhase::Allocating;
                Ok(vec![SessionEffect::Allocate])
            }
            SessionSignal::AllocateReady { now_ms, allocation } => {
                self.require_phase(SessionPhase::Allocating)?;
                self.accept_initial_ready(now_ms, allocation)
            }
            SessionSignal::AllocatePending {
                now_ms,
                pending,
                jitter_percent,
            } => {
                self.require_phase(SessionPhase::Allocating)?;
                validate_jitter(jitter_percent)?;
                self.pending = Some(pending);
                self.phase = SessionPhase::Pending;
                self.schedule_poll(now_ms, jitter_percent)
            }
            SessionSignal::PollPending {
                now_ms,
                pending,
                jitter_percent,
            } => {
                self.require_phase(SessionPhase::Pending)?;
                validate_jitter(jitter_percent)?;
                if self.accept_pending_update(pending)? {
                    return self.fail_protocol_with_known_cleanup();
                }
                self.schedule_poll(now_ms, jitter_percent)
            }
            SessionSignal::PollReady { now_ms, allocation } => {
                self.require_phase(SessionPhase::Pending)?;
                if self.pending_allocation_conflicts(&allocation) {
                    return self.fail_protocol_with_known_cleanup();
                }
                self.accept_initial_ready(now_ms, allocation)
            }
            SessionSignal::Tick { now_ms } => self.tick(now_ms),
            SessionSignal::RenewReady { now_ms, allocation } => {
                self.require_phase(SessionPhase::Renewing)?;
                if self
                    .expires_at_ms
                    .is_some_and(|expires_at_ms| now_ms >= expires_at_ms)
                {
                    return self.finish(TerminalOutcome::Expired);
                }
                let Some(current) = &self.allocation else {
                    return Err(SessionError::InvariantViolation);
                };
                if !current.same_binding_as(&allocation) {
                    return self.renew_failed(now_ms, SessionFailureKind::Protocol);
                }
                self.install_ready(now_ms, allocation)?;
                self.renew_attempts = 0;
                Ok(Vec::new())
            }
            SessionSignal::RenewFailed { now_ms, kind } => {
                self.require_phase(SessionPhase::Renewing)?;
                self.renew_failed(now_ms, kind)
            }
            SessionSignal::StreamStarted { now_ms } => self.start_stream(now_ms),
            SessionSignal::OutputStarted => {
                if !self.stream_started {
                    return Err(SessionError::InvalidTransition {
                        phase: self.phase,
                        signal: "output-started",
                    });
                }
                self.output_started = true;
                Ok(Vec::new())
            }
            SessionSignal::Completed => self.finish(TerminalOutcome::Completed),
            SessionSignal::Cancelled => self.finish(TerminalOutcome::Cancelled),
            SessionSignal::Failed(kind) => self.finish(TerminalOutcome::Failed(kind)),
            SessionSignal::AllocateOutcomeUnknown => {
                self.require_phase(SessionPhase::Allocating)?;
                self.finish_without_release(TerminalOutcome::Failed(
                    SessionFailureKind::AllocationOutcomeUnknown,
                ))
            }
            SessionSignal::DaemonRestarted { now_ms } => self.daemon_restarted(now_ms),
            SessionSignal::ReleaseSucceeded => {
                self.release_finished(SessionCleanupOutcome::Released, None)
            }
            SessionSignal::ReleaseFailed(kind) => {
                self.release_finished(SessionCleanupOutcome::DeferredToTtl, Some(kind))
            }
        }
    }

    fn require_phase(&self, expected: SessionPhase) -> Result<(), SessionError> {
        if self.phase == expected {
            Ok(())
        } else {
            Err(SessionError::InvalidTransition {
                phase: self.phase,
                signal: "phase-specific",
            })
        }
    }

    fn accept_pending_update(&mut self, pending: PendingAllocation) -> Result<bool, SessionError> {
        let Some(current) = &self.pending else {
            return Err(SessionError::InvariantViolation);
        };
        if current.operation_id != pending.operation_id {
            return Ok(true);
        }
        if let (Some(current), Some(next)) = (
            &current.cleanup_allocation_id,
            &pending.cleanup_allocation_id,
        ) {
            if current != next {
                return Ok(true);
            }
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|current| current.cleanup_allocation_id.is_none())
        {
            self.pending = Some(pending);
        }
        Ok(false)
    }

    fn pending_allocation_conflicts(&self, allocation: &ReadyAllocation) -> bool {
        self.pending
            .as_ref()
            .and_then(|pending| pending.cleanup_allocation_id.as_ref())
            .is_some_and(|allocation_id| allocation_id != &allocation.allocation_id)
    }

    fn accept_initial_ready(
        &mut self,
        now_ms: u64,
        allocation: ReadyAllocation,
    ) -> Result<Vec<SessionEffect>, SessionError> {
        self.pending = None;
        self.allocation = Some(allocation);
        if now_ms >= self.startup_deadline_ms {
            return self.finish(TerminalOutcome::Failed(SessionFailureKind::Timeout));
        }
        let allocation = self
            .allocation
            .take()
            .ok_or(SessionError::InvariantViolation)?;
        self.install_ready(now_ms, allocation)?;
        Ok(Vec::new())
    }

    fn install_ready(
        &mut self,
        now_ms: u64,
        allocation: ReadyAllocation,
    ) -> Result<(), SessionError> {
        let ttl_ms = u64::from(allocation.effective_ttl_seconds) * 1_000;
        let renew_after_ms = ttl_ms.checked_mul(4).ok_or(SessionError::TimeOverflow)? / 5;
        self.renew_at_ms = Some(
            now_ms
                .checked_add(renew_after_ms)
                .ok_or(SessionError::TimeOverflow)?,
        );
        self.expires_at_ms = Some(
            now_ms
                .checked_add(ttl_ms)
                .ok_or(SessionError::TimeOverflow)?,
        );
        self.renew_retry_at_ms = None;
        self.allocation = Some(allocation);
        self.phase = SessionPhase::Ready;
        Ok(())
    }

    fn schedule_poll(
        &mut self,
        now_ms: u64,
        jitter_percent: i8,
    ) -> Result<Vec<SessionEffect>, SessionError> {
        validate_jitter(jitter_percent)?;
        if now_ms >= self.startup_deadline_ms {
            return self.finish(TerminalOutcome::Failed(SessionFailureKind::Timeout));
        }
        let base_ms = match self.poll_schedule_index {
            0 => 250,
            1 => 500,
            _ => 1_000,
        };
        self.poll_schedule_index = self.poll_schedule_index.saturating_add(1);
        let adjusted_ms = (i64::from(base_ms) * (100 + i64::from(jitter_percent)) / 100) as u64;
        let after_ms = adjusted_ms.min(self.startup_deadline_ms - now_ms);
        let operation_id = self
            .pending
            .as_ref()
            .map(|pending| pending.operation_id.clone())
            .ok_or(SessionError::InvariantViolation)?;
        Ok(vec![SessionEffect::SchedulePoll {
            operation_id,
            after_ms,
            deadline_ms: self.startup_deadline_ms,
        }])
    }

    fn tick(&mut self, now_ms: u64) -> Result<Vec<SessionEffect>, SessionError> {
        match self.phase {
            SessionPhase::Pending if now_ms >= self.startup_deadline_ms => {
                self.finish(TerminalOutcome::Failed(SessionFailureKind::Timeout))
            }
            SessionPhase::Ready => {
                let expires_at_ms = self.expires_at_ms.ok_or(SessionError::InvariantViolation)?;
                if now_ms >= expires_at_ms {
                    return self.finish(TerminalOutcome::Expired);
                }
                let renew_due = if let Some(retry_at_ms) = self.renew_retry_at_ms {
                    now_ms >= retry_at_ms
                } else {
                    self.renew_at_ms
                        .is_some_and(|renew_at_ms| now_ms >= renew_at_ms)
                };
                if !renew_due {
                    return Ok(Vec::new());
                }
                let allocation_id = self
                    .allocation
                    .as_ref()
                    .map(|allocation| allocation.allocation_id.clone())
                    .ok_or(SessionError::InvariantViolation)?;
                self.phase = SessionPhase::Renewing;
                self.renew_retry_at_ms = None;
                self.renew_attempts = self.renew_attempts.saturating_add(1);
                Ok(vec![SessionEffect::Renew { allocation_id }])
            }
            _ => Ok(Vec::new()),
        }
    }

    fn renew_failed(
        &mut self,
        now_ms: u64,
        kind: SessionFailureKind,
    ) -> Result<Vec<SessionEffect>, SessionError> {
        if self
            .expires_at_ms
            .is_some_and(|expires_at_ms| now_ms >= expires_at_ms)
        {
            return self.finish(TerminalOutcome::Expired);
        }
        if !self.stream_started {
            if kind.permits_pre_stream_reallocation()
                && !self.reallocated
                && now_ms < self.startup_deadline_ms
            {
                self.reallocated = true;
                self.clear_allocation();
                self.phase = SessionPhase::Allocating;
                return Ok(vec![SessionEffect::Allocate]);
            }
            return self.finish(TerminalOutcome::Failed(kind));
        }

        let expires_at_ms = self.expires_at_ms.ok_or(SessionError::InvariantViolation)?;
        let retry_at_ms = now_ms
            .checked_add(STREAM_RENEW_RETRY_DELAY_MS)
            .ok_or(SessionError::TimeOverflow)?;
        self.phase = SessionPhase::Ready;
        self.renew_at_ms = None;
        if kind.permits_stream_renew_retry()
            && self.renew_attempts < STREAM_RENEW_MAX_ATTEMPTS
            && retry_at_ms < expires_at_ms
        {
            self.renew_retry_at_ms = Some(retry_at_ms);
        } else {
            self.renew_retry_at_ms = None;
        }
        Ok(Vec::new())
    }

    fn start_stream(&mut self, now_ms: u64) -> Result<Vec<SessionEffect>, SessionError> {
        self.require_phase(SessionPhase::Ready)?;
        if self.stream_started {
            return Ok(Vec::new());
        }
        if self
            .expires_at_ms
            .is_some_and(|expires_at_ms| now_ms >= expires_at_ms)
        {
            return self.finish(TerminalOutcome::Expired);
        }
        let allocation = self
            .allocation
            .as_ref()
            .ok_or(SessionError::InvariantViolation)?;
        self.stream_started = true;
        Ok(vec![SessionEffect::BeginStream {
            allocation_id: allocation.allocation_id.clone(),
            binding_fingerprint: allocation.binding_fingerprint.clone(),
        }])
    }

    fn daemon_restarted(&mut self, now_ms: u64) -> Result<Vec<SessionEffect>, SessionError> {
        if self.terminal_outcome.is_some() || matches!(self.phase, SessionPhase::Idle) {
            return Ok(Vec::new());
        }
        if !self.stream_started && !self.reallocated && now_ms < self.startup_deadline_ms {
            self.reallocated = true;
            self.pending = None;
            self.clear_allocation();
            self.phase = SessionPhase::Allocating;
            return Ok(vec![SessionEffect::Allocate]);
        }
        self.finish_without_release(TerminalOutcome::Failed(SessionFailureKind::AllocationLost))
    }

    fn clear_allocation(&mut self) {
        self.allocation = None;
        self.renew_at_ms = None;
        self.renew_retry_at_ms = None;
        self.expires_at_ms = None;
        self.renew_attempts = 0;
    }

    fn fail_protocol_with_known_cleanup(&mut self) -> Result<Vec<SessionEffect>, SessionError> {
        self.finish(TerminalOutcome::Failed(SessionFailureKind::Protocol))
    }

    fn release_target(&self) -> Option<BoundedIdentifier> {
        self.allocation
            .as_ref()
            .map(|allocation| allocation.allocation_id.clone())
            .or_else(|| {
                self.pending
                    .as_ref()
                    .and_then(|pending| pending.cleanup_allocation_id.clone())
            })
    }

    fn finish(&mut self, outcome: TerminalOutcome) -> Result<Vec<SessionEffect>, SessionError> {
        if self.terminal_outcome.is_some() {
            return Ok(Vec::new());
        }
        self.terminal_outcome = Some(outcome);
        if let Some(allocation_id) = self.release_target() {
            self.release_requested = true;
            self.cleanup_outcome = SessionCleanupOutcome::Pending;
            self.phase = if outcome == TerminalOutcome::Expired {
                SessionPhase::Expired
            } else {
                SessionPhase::Releasing
            };
            return Ok(vec![SessionEffect::Release { allocation_id }]);
        }
        self.cleanup_outcome = SessionCleanupOutcome::NotStarted;
        self.phase = terminal_phase(outcome);
        Ok(Vec::new())
    }

    fn finish_without_release(
        &mut self,
        outcome: TerminalOutcome,
    ) -> Result<Vec<SessionEffect>, SessionError> {
        if self.terminal_outcome.is_some() {
            return Ok(Vec::new());
        }
        self.terminal_outcome = Some(outcome);
        self.cleanup_outcome = SessionCleanupOutcome::DeferredToTtl;
        self.phase = terminal_phase(outcome);
        Ok(Vec::new())
    }

    fn release_finished(
        &mut self,
        cleanup_outcome: SessionCleanupOutcome,
        release_failure_kind: Option<ReleaseFailureKind>,
    ) -> Result<Vec<SessionEffect>, SessionError> {
        if self.release_finished {
            return Ok(Vec::new());
        }
        if !self.release_requested {
            return Err(SessionError::InvalidTransition {
                phase: self.phase,
                signal: "release-result",
            });
        }
        self.release_finished = true;
        self.cleanup_outcome = cleanup_outcome;
        self.release_failure_kind = release_failure_kind;
        if self.terminal_outcome != Some(TerminalOutcome::Expired) {
            self.phase = SessionPhase::Released;
        }
        Ok(Vec::new())
    }
}

fn terminal_phase(outcome: TerminalOutcome) -> SessionPhase {
    match outcome {
        TerminalOutcome::Failed(_) => SessionPhase::Failed,
        TerminalOutcome::Expired => SessionPhase::Expired,
        TerminalOutcome::Completed | TerminalOutcome::Cancelled => SessionPhase::Released,
    }
}

fn validate_jitter(jitter_percent: i8) -> Result<(), SessionError> {
    if (-MAX_JITTER_PERCENT..=MAX_JITTER_PERCENT).contains(&jitter_percent) {
        Ok(())
    } else {
        Err(SessionError::InvalidJitter)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionError {
    Contract(ContractError),
    InvalidStartupTimeout,
    InvalidJitter,
    TimeOverflow,
    InvariantViolation,
    InvalidTransition {
        phase: SessionPhase,
        signal: &'static str,
    },
}

impl From<ContractError> for SessionError {
    fn from(error: ContractError) -> Self {
        Self::Contract(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::larm::contracts::SelectionReason;

    fn ready(allocation_id: &str, runtime_id: &str, binding: &str) -> ReadyAllocation {
        ReadyAllocation::new(
            allocation_id,
            runtime_id,
            binding,
            60,
            false,
            SelectionReason::Primary,
        )
        .expect("ready allocation fixture is valid")
    }

    fn started_session() -> AllocationSession {
        let mut session = AllocationSession::new(0, 30).expect("session initializes");
        assert_eq!(
            session.transition(SessionSignal::Start),
            Ok(vec![SessionEffect::Allocate])
        );
        session
    }

    #[test]
    fn immediate_ready_uses_server_ttl_and_monotonic_renew_schedule() {
        let mut session = started_session();
        assert!(session
            .transition(SessionSignal::AllocateReady {
                now_ms: 1_000,
                allocation: ready("allocation_1", "runtime_1", "binding_1"),
            })
            .expect("200 ready response is accepted")
            .is_empty());
        assert_eq!(session.phase(), SessionPhase::Ready);
        assert_eq!(session.renew_at_ms(), Some(49_000));
        assert_eq!(session.expires_at_ms(), Some(61_000));
        assert_eq!(session.startup_deadline_ms(), 30_000);
    }

    #[test]
    fn pending_poll_backoff_is_bounded_jittered_and_deterministic() {
        let mut session = started_session();
        let pending = PendingAllocation::new("operation_1", Some("allocation_1".to_string()))
            .expect("pending fixture is valid");
        let first = session
            .transition(SessionSignal::AllocatePending {
                now_ms: 100,
                pending: pending.clone(),
                jitter_percent: 0,
            })
            .expect("202 response is accepted");
        assert_eq!(
            first,
            [SessionEffect::SchedulePoll {
                operation_id: pending.operation_id.clone(),
                after_ms: 250,
                deadline_ms: 30_000,
            }]
        );
        let second = session
            .transition(SessionSignal::PollPending {
                now_ms: 350,
                pending: pending.clone(),
                jitter_percent: 20,
            })
            .expect("pending poll response is accepted");
        assert_eq!(
            second,
            [SessionEffect::SchedulePoll {
                operation_id: pending.operation_id.clone(),
                after_ms: 600,
                deadline_ms: 30_000,
            }]
        );
        let third = session
            .transition(SessionSignal::PollPending {
                now_ms: 950,
                pending,
                jitter_percent: -20,
            })
            .expect("third pending response is accepted");
        assert!(matches!(
            third.as_slice(),
            [SessionEffect::SchedulePoll { after_ms: 800, .. }]
        ));
        assert!(session
            .transition(SessionSignal::PollReady {
                now_ms: 1_750,
                allocation: ready("allocation_1", "runtime_1", "binding_1"),
            })
            .expect("poll becomes ready")
            .is_empty());
        assert_eq!(session.phase(), SessionPhase::Ready);
    }

    #[test]
    fn renew_preserves_allocation_and_binding_and_recomputes_ttl() {
        let mut session = started_session();
        session
            .transition(SessionSignal::AllocateReady {
                now_ms: 0,
                allocation: ready("allocation_1", "runtime_1", "binding_1"),
            })
            .expect("allocation becomes ready");
        assert_eq!(
            session.transition(SessionSignal::Tick { now_ms: 48_000 }),
            Ok(vec![SessionEffect::Renew {
                allocation_id: BoundedIdentifier::new("allocation_1")
                    .expect("identifier fixture is valid"),
            }])
        );
        let renewed = ReadyAllocation::new(
            "allocation_1",
            "runtime_1",
            "binding_1",
            120,
            false,
            SelectionReason::Other,
        )
        .expect("renew fixture is valid");
        session
            .transition(SessionSignal::RenewReady {
                now_ms: 48_100,
                allocation: renewed,
            })
            .expect("renew succeeds");
        assert_eq!(session.phase(), SessionPhase::Ready);
        assert_eq!(session.renew_at_ms(), Some(144_100));
        assert_eq!(session.expires_at_ms(), Some(168_100));
    }

    #[test]
    fn stream_started_binding_change_is_rejected_without_stopping_the_existing_stream() {
        let mut session = started_session();
        session
            .transition(SessionSignal::AllocateReady {
                now_ms: 0,
                allocation: ready("allocation_1", "runtime_1", "binding_1"),
            })
            .expect("allocation becomes ready");
        session
            .transition(SessionSignal::StreamStarted { now_ms: 1 })
            .expect("stream starts");
        session
            .transition(SessionSignal::OutputStarted)
            .expect("first output is recorded");
        session
            .transition(SessionSignal::OutputStarted)
            .expect("duplicate output-started signal is idempotent");
        assert!(session.output_started());
        session
            .transition(SessionSignal::Tick { now_ms: 48_000 })
            .expect("renew starts");
        let effects = session
            .transition(SessionSignal::RenewReady {
                now_ms: 48_100,
                allocation: ready("allocation_1", "runtime_2", "binding_2"),
            })
            .expect("binding mismatch keeps the existing stream binding");
        assert!(effects.is_empty());
        assert_eq!(session.phase(), SessionPhase::Ready);
        assert_eq!(session.terminal_outcome(), None);
        assert_eq!(session.renew_at_ms(), None);
        assert_eq!(
            session
                .allocation()
                .expect("original allocation remains installed")
                .selected_runtime_id
                .as_str(),
            "runtime_1"
        );
        assert!(session.stream_started());
    }

    #[test]
    fn pre_stream_binding_change_is_a_protocol_terminal() {
        let mut session = started_session();
        session
            .transition(SessionSignal::AllocateReady {
                now_ms: 0,
                allocation: ready("allocation_1", "runtime_1", "binding_1"),
            })
            .expect("allocation becomes ready");
        session
            .transition(SessionSignal::Tick { now_ms: 48_000 })
            .expect("renew starts");
        let effects = session
            .transition(SessionSignal::RenewReady {
                now_ms: 48_100,
                allocation: ready("allocation_1", "runtime_2", "binding_2"),
            })
            .expect("pre-stream binding mismatch becomes terminal");
        assert!(matches!(
            effects.as_slice(),
            [SessionEffect::Release { .. }]
        ));
        assert_eq!(
            session.terminal_outcome(),
            Some(TerminalOutcome::Failed(SessionFailureKind::Protocol))
        );
    }

    #[test]
    fn stream_renew_retries_twice_without_changing_binding() {
        let mut session = started_session();
        session
            .transition(SessionSignal::AllocateReady {
                now_ms: 0,
                allocation: ready("allocation_1", "runtime_1", "binding_1"),
            })
            .expect("allocation becomes ready");
        session
            .transition(SessionSignal::StreamStarted { now_ms: 1 })
            .expect("stream starts");
        session
            .transition(SessionSignal::Tick { now_ms: 48_000 })
            .expect("first renew starts");
        session
            .transition(SessionSignal::RenewFailed {
                now_ms: 48_100,
                kind: SessionFailureKind::Network,
            })
            .expect("first renew failure schedules one retry");
        assert_eq!(session.renew_at_ms(), Some(49_100));
        assert!(session
            .transition(SessionSignal::Tick { now_ms: 49_099 })
            .expect("early tick is ignored")
            .is_empty());
        assert!(matches!(
            session
                .transition(SessionSignal::Tick { now_ms: 49_100 })
                .expect("second renew starts")
                .as_slice(),
            [SessionEffect::Renew { .. }]
        ));
        session
            .transition(SessionSignal::RenewFailed {
                now_ms: 49_200,
                kind: SessionFailureKind::Network,
            })
            .expect("second renew failure does not change the stream binding");
        assert_eq!(session.phase(), SessionPhase::Ready);
        assert_eq!(session.renew_at_ms(), None);
        assert_eq!(
            session
                .allocation()
                .expect("allocation remains fixed")
                .binding_fingerprint
                .as_str(),
            "binding_1"
        );
    }

    #[test]
    fn pre_stream_renew_failure_reallocates_once_within_startup_deadline() {
        let mut session = AllocationSession::new(0, 300).expect("session initializes");
        session
            .transition(SessionSignal::Start)
            .expect("allocation starts");
        session
            .transition(SessionSignal::AllocateReady {
                now_ms: 0,
                allocation: ready("allocation_old", "runtime_old", "binding_old"),
            })
            .expect("old allocation becomes ready");
        session
            .transition(SessionSignal::Tick { now_ms: 48_000 })
            .expect("renew starts before streaming");
        assert_eq!(
            session.transition(SessionSignal::RenewFailed {
                now_ms: 48_100,
                kind: SessionFailureKind::Network,
            }),
            Ok(vec![SessionEffect::Allocate])
        );
        assert!(session.allocation().is_none());
        session
            .transition(SessionSignal::AllocateReady {
                now_ms: 49_000,
                allocation: ready("allocation_new", "runtime_new", "binding_new"),
            })
            .expect("replacement allocation becomes ready");
        session
            .transition(SessionSignal::Tick { now_ms: 97_000 })
            .expect("replacement renew starts");
        let effects = session
            .transition(SessionSignal::RenewFailed {
                now_ms: 97_100,
                kind: SessionFailureKind::Network,
            })
            .expect("second reallocation is refused");
        assert!(matches!(
            effects.as_slice(),
            [SessionEffect::Release { .. }]
        ));
        assert_eq!(
            session.terminal_outcome(),
            Some(TerminalOutcome::Failed(SessionFailureKind::Network))
        );
    }

    #[test]
    fn complete_and_release_are_one_shot_and_idempotent() {
        let mut session = started_session();
        session
            .transition(SessionSignal::AllocateReady {
                now_ms: 0,
                allocation: ready("allocation_1", "runtime_1", "binding_1"),
            })
            .expect("allocation becomes ready");
        let first = session
            .transition(SessionSignal::Completed)
            .expect("completion requests release");
        assert!(matches!(first.as_slice(), [SessionEffect::Release { .. }]));
        assert!(session
            .transition(SessionSignal::Completed)
            .expect("duplicate completion is ignored")
            .is_empty());
        assert!(session
            .transition(SessionSignal::ReleaseSucceeded)
            .expect("release completes")
            .is_empty());
        assert!(session
            .transition(SessionSignal::ReleaseSucceeded)
            .expect("duplicate release completion is ignored")
            .is_empty());
        assert_eq!(session.phase(), SessionPhase::Released);
        assert_eq!(session.cleanup_outcome(), SessionCleanupOutcome::Released);
    }

    #[test]
    fn pending_cancel_releases_the_verified_allocation_once() {
        let mut session = started_session();
        session
            .transition(SessionSignal::AllocatePending {
                now_ms: 0,
                pending: PendingAllocation::new("operation_1", Some("allocation_1".to_string()))
                    .expect("pending fixture is valid"),
                jitter_percent: 0,
            })
            .expect("allocation becomes pending");
        let effects = session
            .transition(SessionSignal::Cancelled)
            .expect("cancel requests cleanup");
        assert!(matches!(
            effects.as_slice(),
            [SessionEffect::Release { .. }]
        ));
        assert!(session
            .transition(SessionSignal::Cancelled)
            .expect("duplicate cancel is ignored")
            .is_empty());
        assert_eq!(session.terminal_outcome(), Some(TerminalOutcome::Cancelled));
    }

    #[test]
    fn expiry_is_terminal_and_release_failure_is_deferred_to_ttl() {
        let mut session = started_session();
        session
            .transition(SessionSignal::AllocateReady {
                now_ms: 0,
                allocation: ready("allocation_1", "runtime_1", "binding_1"),
            })
            .expect("allocation becomes ready");
        assert!(matches!(
            session
                .transition(SessionSignal::Tick { now_ms: 60_000 })
                .expect("expiry is observed")
                .as_slice(),
            [SessionEffect::Release { .. }]
        ));
        assert_eq!(session.phase(), SessionPhase::Expired);
        session
            .transition(SessionSignal::ReleaseFailed(ReleaseFailureKind::Network))
            .expect("release failure is bounded");
        assert_eq!(session.phase(), SessionPhase::Expired);
        assert_eq!(
            session.cleanup_outcome(),
            SessionCleanupOutcome::DeferredToTtl
        );
        assert_eq!(
            session.release_failure_kind(),
            Some(ReleaseFailureKind::Network)
        );
    }

    #[test]
    fn pending_startup_deadline_stops_polling_and_releases_known_allocation() {
        let mut session = AllocationSession::new(0, 1).expect("session initializes");
        session
            .transition(SessionSignal::Start)
            .expect("allocation starts");
        session
            .transition(SessionSignal::AllocatePending {
                now_ms: 0,
                pending: PendingAllocation::new("operation_1", Some("allocation_1".to_string()))
                    .expect("pending fixture is valid"),
                jitter_percent: 0,
            })
            .expect("allocation becomes pending");
        let effects = session
            .transition(SessionSignal::Tick { now_ms: 1_000 })
            .expect("deadline becomes terminal");
        assert!(matches!(
            effects.as_slice(),
            [SessionEffect::Release { .. }]
        ));
        assert_eq!(
            session.terminal_outcome(),
            Some(TerminalOutcome::Failed(SessionFailureKind::Timeout))
        );
    }

    #[test]
    fn unknown_allocate_outcome_never_blind_retries() {
        let mut session = started_session();
        assert!(session
            .transition(SessionSignal::AllocateOutcomeUnknown)
            .expect("unknown outcome becomes terminal")
            .is_empty());
        assert_eq!(session.phase(), SessionPhase::Failed);
        assert_eq!(
            session.terminal_outcome(),
            Some(TerminalOutcome::Failed(
                SessionFailureKind::AllocationOutcomeUnknown
            ))
        );
        assert_eq!(
            session.cleanup_outcome(),
            SessionCleanupOutcome::DeferredToTtl
        );
    }

    #[test]
    fn daemon_restart_discards_old_allocation_and_reallocates_only_once() {
        let mut session = started_session();
        session
            .transition(SessionSignal::AllocateReady {
                now_ms: 0,
                allocation: ready("allocation_old", "runtime_old", "binding_old"),
            })
            .expect("old allocation becomes ready");
        assert_eq!(
            session.transition(SessionSignal::DaemonRestarted { now_ms: 500 }),
            Ok(vec![SessionEffect::Allocate])
        );
        assert!(session.allocation().is_none());
        session
            .transition(SessionSignal::AllocateReady {
                now_ms: 1_000,
                allocation: ready("allocation_new", "runtime_new", "binding_new"),
            })
            .expect("replacement allocation becomes ready");
        assert!(session
            .transition(SessionSignal::DaemonRestarted { now_ms: 1_500 })
            .expect("second restart becomes terminal")
            .is_empty());
        assert_eq!(session.phase(), SessionPhase::Failed);
        assert_eq!(
            session.terminal_outcome(),
            Some(TerminalOutcome::Failed(SessionFailureKind::AllocationLost))
        );
    }

    #[test]
    fn pending_allocation_identity_change_is_rejected_before_gateway_use() {
        let mut session = started_session();
        session
            .transition(SessionSignal::AllocatePending {
                now_ms: 0,
                pending: PendingAllocation::new("operation_1", Some("allocation_1".to_string()))
                    .expect("pending fixture is valid"),
                jitter_percent: 0,
            })
            .expect("allocation becomes pending");
        let effects = session
            .transition(SessionSignal::PollReady {
                now_ms: 250,
                allocation: ready("allocation_2", "runtime_1", "binding_1"),
            })
            .expect("identity mismatch becomes a bounded failure");
        assert!(matches!(
            effects.as_slice(),
            [SessionEffect::Release { .. }]
        ));
        assert_eq!(
            session.terminal_outcome(),
            Some(TerminalOutcome::Failed(SessionFailureKind::Protocol))
        );
    }

    #[test]
    fn contracts_reject_unbounded_ids_ttl_and_fallback() {
        assert_eq!(
            ReadyAllocation::new(
                "bad id",
                "runtime_1",
                "binding_1",
                60,
                false,
                SelectionReason::Primary,
            ),
            Err(ContractError::InvalidIdentifier)
        );
        assert_eq!(
            ReadyAllocation::new(
                "allocation_1",
                "runtime_1",
                "binding_1",
                59,
                false,
                SelectionReason::Primary,
            ),
            Err(ContractError::InvalidEffectiveTtl)
        );
        assert_eq!(
            ReadyAllocation::new(
                "allocation_1",
                "runtime_1",
                "binding_1",
                60,
                true,
                SelectionReason::Primary,
            ),
            Err(ContractError::FallbackNotAllowed)
        );
    }
}
