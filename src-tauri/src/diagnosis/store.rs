use super::contract::{DiagnosisReport, DiagnosisStatus};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use tokio::sync::Notify;

pub(crate) struct DiagnosisStore {
    latest: Mutex<DiagnosisReport>,
    running: AtomicBool,
    revision: AtomicU64,
    finished: Notify,
}

impl DiagnosisStore {
    pub(crate) fn new() -> Self {
        Self {
            latest: Mutex::new(DiagnosisReport {
                revision: 0,
                started_at: String::new(),
                finished_at: None,
                running: false,
                overall: DiagnosisStatus::Skipped,
                items: Vec::new(),
            }),
            running: AtomicBool::new(false),
            revision: AtomicU64::new(0),
            finished: Notify::new(),
        }
    }

    pub(crate) fn snapshot(&self) -> DiagnosisReport {
        let mut report = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        report.running = self.running.load(Ordering::SeqCst);
        report
    }

    pub(crate) fn try_begin(&self) -> Option<u64> {
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return None;
        }
        Some(self.revision.fetch_add(1, Ordering::SeqCst) + 1)
    }

    pub(crate) fn publish(&self, mut report: DiagnosisReport) {
        report.running = false;
        *self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = report;
        self.running.store(false, Ordering::SeqCst);
        self.finished.notify_waiters();
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

    #[test]
    fn dg_02_try_begin_is_single_flight() {
        let store = DiagnosisStore::new();
        assert_eq!(store.try_begin(), Some(1));
        assert_eq!(store.try_begin(), None);
    }

    #[test]
    fn dg_02_publish_clears_running_and_bumps_revision() {
        let store = DiagnosisStore::new();
        let revision = store.try_begin().expect("first run starts");
        let mut report = store.snapshot();
        report.revision = revision;
        report.overall = DiagnosisStatus::Ok;
        store.publish(report);
        let snapshot = store.snapshot();
        assert!(!snapshot.running);
        assert_eq!(snapshot.revision, 1);
        assert_eq!(store.try_begin(), Some(2));
    }
}
