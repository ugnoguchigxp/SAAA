use super::aggregate;
use super::contract::{now_ms, Capability, DiagnosisReport, Evidence, SCHEMA_VERSION};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tokio::sync::Notify;

#[derive(Default)]
struct Inner {
    revision: u64,
    started_at: u64,
    finished_at: Option<u64>,
    /// Latest evidence per check. A run replaces only the checks it executed, so
    /// older results stay visible until they expire.
    results: BTreeMap<&'static str, Vec<Evidence>>,
    /// Set while a single-capability run is active.
    focus: Option<Capability>,
}

fn build(inner: &Inner, running: bool, now: u64) -> DiagnosisReport {
    let evidence: Vec<Evidence> = inner.results.values().flatten().cloned().collect();
    let capabilities = aggregate::all_capabilities(&evidence, now);
    DiagnosisReport {
        schema_version: SCHEMA_VERSION,
        revision: inner.revision,
        started_at: inner.started_at,
        finished_at: inner.finished_at,
        running,
        overall: aggregate::overall(&capabilities),
        capabilities,
    }
}

pub(crate) struct DiagnosisStore {
    inner: Mutex<Inner>,
    running: AtomicBool,
    finished: Notify,
}

impl DiagnosisStore {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            running: AtomicBool::new(false),
            finished: Notify::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn snapshot(&self) -> DiagnosisReport {
        self.snapshot_at(now_ms())
    }

    pub(crate) fn snapshot_at(&self, now: u64) -> DiagnosisReport {
        let inner = self.lock();
        build(&inner, self.running.load(Ordering::SeqCst), now)
    }

    /// Single flight: returns the new revision, or `None` while another run is active.
    pub(crate) fn try_begin(&self) -> Option<u64> {
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return None;
        }
        let mut inner = self.lock();
        inner.revision += 1;
        inner.started_at = now_ms();
        inner.finished_at = None;
        Some(inner.revision)
    }

    pub(crate) fn set_focus(&self, focus: Option<Capability>) {
        self.lock().focus = focus;
    }

    pub(crate) fn focus(&self) -> Option<Capability> {
        self.lock().focus
    }

    /// Replaces a check's evidence. During a focused run only the focused capability is
    /// replaced; evidence the check produced earlier for other capabilities is kept.
    pub(crate) fn put(&self, check: &'static str, mut evidence: Vec<Evidence>) {
        let mut inner = self.lock();
        if let Some(focus) = inner.focus {
            if let Some(old) = inner.results.get(check) {
                let kept: Vec<Evidence> = old
                    .iter()
                    .filter(|item| {
                        item.capability != focus
                            && !evidence.iter().any(|new| {
                                new.source == item.source
                                    && new.route == item.route
                                    && new.capability == item.capability
                            })
                    })
                    .cloned()
                    .collect();
                evidence.extend(kept);
            }
        }
        inner.results.insert(check, evidence);
    }

    /// Ends the run and returns its final report, built before a waiting run can start.
    pub(crate) fn finish(&self) -> DiagnosisReport {
        let report = {
            let mut inner = self.lock();
            let now = now_ms();
            inner.finished_at = Some(now);
            inner.focus = None;
            build(&inner, false, now)
        };
        self.running.store(false, Ordering::SeqCst);
        self.finished.notify_waiters();
        report
    }

    pub(crate) async fn wait_finished(&self) {
        loop {
            let notified = self.finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.running.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::contract::{
        Capability, CapabilityState, Importance, Outcome, Reason, Tier,
    };

    fn pass(source: &str, capability: Capability) -> Evidence {
        Evidence {
            source: source.into(),
            capability,
            route: "core".into(),
            tier: Tier::Static,
            importance: Importance::Required,
            outcome: Outcome::Pass,
            reason: Reason::Ok,
            subject: None,
            detail: None,
            latency_ms: None,
            observed_at: 1,
            expires_at: None,
        }
    }

    #[test]
    fn begin_is_single_flight_and_finish_bumps_nothing_but_unlocks() {
        let store = DiagnosisStore::new();
        assert_eq!(store.try_begin(), Some(1));
        assert_eq!(store.try_begin(), None);
        assert!(store.snapshot().running);
        store.finish();
        let snapshot = store.snapshot();
        assert!(!snapshot.running);
        assert!(snapshot.finished_at.is_some());
        assert_eq!(store.try_begin(), Some(2));
    }

    #[test]
    fn a_run_replaces_only_its_own_checks() {
        let store = DiagnosisStore::new();
        store.try_begin();
        store.put("a", vec![pass("a", Capability::Storage)]);
        store.put("b", vec![pass("b", Capability::Storage)]);
        store.finish();
        store.try_begin();
        store.put("a", Vec::new());
        store.finish();
        let storage = store
            .snapshot()
            .capabilities
            .into_iter()
            .find(|item| item.capability == Capability::Storage)
            .unwrap();
        assert_eq!(storage.evidence.len(), 1);
        assert_eq!(storage.evidence[0].source, "b");
        assert_eq!(storage.state, CapabilityState::Ready);
    }

    #[test]
    fn a_focused_run_keeps_other_capabilities_evidence_of_the_same_check() {
        let mut other = pass("p", Capability::Memory);
        other.source = "keep".into();
        let store = DiagnosisStore::new();
        store.try_begin();
        store.put("probe", vec![pass("p", Capability::Storage), other]);
        store.finish();
        store.try_begin();
        store.set_focus(Some(Capability::Storage));
        store.put("probe", vec![pass("p2", Capability::Storage)]);
        store.finish();
        assert_eq!(store.focus(), None);
        let report = store.snapshot();
        let sources = |capability: Capability| {
            report
                .capabilities
                .iter()
                .find(|item| item.capability == capability)
                .unwrap()
                .evidence
                .iter()
                .map(|item| item.source.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(sources(Capability::Storage), vec!["p2"]);
        assert_eq!(sources(Capability::Memory), vec!["keep"]);
    }

    #[test]
    fn empty_store_reports_every_capability_unverified() {
        let report = DiagnosisStore::new().snapshot();
        assert_eq!(report.revision, 0);
        assert_eq!(report.overall, CapabilityState::Unverified);
        assert_eq!(report.capabilities.len(), Capability::ALL.len());
    }
}
