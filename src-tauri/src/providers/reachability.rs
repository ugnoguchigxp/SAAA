//! LAN 上の Provider Harness への到達性を観測する。選択ロジックへは snapshot だけを渡す。
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Reachability {
    #[default]
    Unknown,
    Reachable,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReachabilitySnapshot {
    pub(crate) harness: Reachability,
    pub(crate) observed_at: Option<Instant>,
    pub(crate) consecutive_failures: u8,
}

impl Default for ReachabilitySnapshot {
    fn default() -> Self {
        Self {
            harness: Reachability::Unknown,
            observed_at: None,
            consecutive_failures: 0,
        }
    }
}

#[derive(Debug)]
struct Inner {
    harness: Reachability,
    observed_at: Option<Instant>,
    consecutive_failures: u8,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            harness: Reachability::Unknown,
            observed_at: None,
            consecutive_failures: 0,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ReachabilityState {
    inner: RwLock<Inner>,
}

impl ReachabilityState {
    pub(crate) fn snapshot(&self) -> ReachabilitySnapshot {
        let inner = read_inner(&self.inner);
        ReachabilitySnapshot {
            harness: inner.harness,
            observed_at: inner.observed_at,
            consecutive_failures: inner.consecutive_failures,
        }
    }

    /// 成功 1 回で Reachable、失敗は FAILURE_THRESHOLD(=2) 連続で Unreachable。
    pub(crate) fn record(&self, ok: bool, now: Instant) {
        let mut inner = write_inner(&self.inner);
        inner.observed_at = Some(now);
        if ok {
            inner.harness = Reachability::Reachable;
            inner.consecutive_failures = 0;
            return;
        }
        inner.consecutive_failures = inner.consecutive_failures.saturating_add(1);
        if inner.harness == Reachability::Unreachable
            || inner.consecutive_failures >= FAILURE_THRESHOLD
        {
            inner.harness = Reachability::Unreachable;
        }
    }

    /// 起動直後・ネットワーク変化直後に Unknown へ戻す。
    pub(crate) fn invalidate(&self) {
        *write_inner(&self.inner) = Inner::default();
    }
}

fn read_inner(lock: &RwLock<Inner>) -> RwLockReadGuard<'_, Inner> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_inner(lock: &RwLock<Inner>) -> RwLockWriteGuard<'_, Inner> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) const FAILURE_THRESHOLD: u8 = 2;
pub(crate) const PROBE_TIMEOUT_MS: u64 = 400;
pub(crate) const PROBE_INTERVAL_SECS: u64 = 20;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_ls_01_two_failures_flip_to_unreachable() {
        let state = ReachabilityState::default();
        let now = Instant::now();
        state.record(true, now);
        assert_eq!(state.snapshot().harness, Reachability::Reachable);
        state.record(false, now);
        assert_eq!(state.snapshot().harness, Reachability::Reachable);
        assert_eq!(state.snapshot().consecutive_failures, 1);
        state.record(false, now);
        assert_eq!(state.snapshot().harness, Reachability::Unreachable);
        assert_eq!(state.snapshot().consecutive_failures, FAILURE_THRESHOLD);
    }

    #[test]
    fn rr_ls_02_one_success_restores() {
        let state = ReachabilityState::default();
        let now = Instant::now();
        state.record(false, now);
        state.record(false, now);
        assert_eq!(state.snapshot().harness, Reachability::Unreachable);
        state.record(true, now);
        let snapshot = state.snapshot();
        assert_eq!(snapshot.harness, Reachability::Reachable);
        assert_eq!(snapshot.consecutive_failures, 0);
    }

    #[test]
    fn rr_ls_03_invalidate_returns_unknown() {
        let state = ReachabilityState::default();
        state.record(true, Instant::now());
        state.invalidate();
        assert_eq!(state.snapshot(), ReachabilitySnapshot::default());
    }
}
