//! Runs the registered checks concurrently, each under its own deadline, and publishes the
//! aggregated report after every check completes. One hung check never hides the others.
use super::checks::{self, CheckSpec};
use super::contract::{
    now_ms, Capability, DiagnosisReport, DiagnosisScope, Evidence, Outcome, Reason, Tier,
};
use super::store::DiagnosisStore;
use crate::AppState;
use futures_util::stream::{FuturesUnordered, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager};

const BACKGROUND_INTERVAL: Duration = Duration::from_secs(5 * 60);

pub(crate) fn spawn_startup(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Keeps recorded-use and catalog evidence fresh without sending provider requests.
        // Each pass is its own task so a panicking check cannot end the loop; the run guard
        // unlocks the store when that happens.
        loop {
            let pass = app.clone();
            let _ = tauri::async_runtime::spawn(async move {
                run_and_publish(&pass, DiagnosisScope::Quick).await;
            })
            .await;
            tokio::time::sleep(BACKGROUND_INTERVAL).await;
        }
    });
}

pub(crate) async fn run_and_publish(
    app: &tauri::AppHandle,
    scope: DiagnosisScope,
) -> DiagnosisReport {
    let state = app.state::<AppState>();
    let store = Arc::clone(&state.diagnosis);
    while store.try_begin().is_none() {
        store.wait_finished().await;
    }
    let guard = RunGuard::new(Arc::clone(&store));
    store.set_focus(match scope {
        DiagnosisScope::Capability { capability } => Some(capability),
        _ => None,
    });
    let emit = |report: &DiagnosisReport| {
        let _ = app.emit("diagnosis-updated", report);
    };
    emit(&store.snapshot());
    execute(&state, &store, &select(scope), emit).await;
    let report = guard.finish();
    emit(&report);
    eprintln!(
        "self diagnosis published revision={} overall={}",
        report.revision,
        report.overall.as_str()
    );
    report
}

/// Unlocks the store even if the run future is dropped mid-flight.
struct RunGuard {
    store: Arc<DiagnosisStore>,
    done: bool,
}

impl RunGuard {
    fn new(store: Arc<DiagnosisStore>) -> Self {
        Self { store, done: false }
    }

    /// Unlocks and returns the final report of this run, taken before a waiting run can begin.
    fn finish(mut self) -> DiagnosisReport {
        self.done = true;
        self.store.finish()
    }
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.store.finish();
        }
    }
}

pub(super) fn select(scope: DiagnosisScope) -> Vec<&'static CheckSpec> {
    checks::REGISTRY
        .iter()
        .filter(|spec| match scope {
            DiagnosisScope::Quick => spec.quick,
            DiagnosisScope::Full => true,
            DiagnosisScope::Capability { capability } => spec.capabilities.contains(&capability),
        })
        .collect()
}

pub(super) async fn execute(
    state: &AppState,
    store: &DiagnosisStore,
    specs: &[&'static CheckSpec],
    mut on_progress: impl FnMut(&DiagnosisReport),
) {
    let mut pending: FuturesUnordered<_> =
        specs.iter().map(|spec| run_check(state, spec)).collect();
    while let Some((spec, evidence)) = pending.next().await {
        store.put(spec.id, evidence);
        on_progress(&store.snapshot());
    }
}

async fn run_check(
    state: &AppState,
    spec: &'static CheckSpec,
) -> (&'static CheckSpec, Vec<Evidence>) {
    let mut evidence = match tokio::time::timeout(spec.timeout, (spec.run)(state)).await {
        Ok(evidence) => evidence,
        // A focused retest must not degrade capabilities it was not asked about.
        Err(_) => {
            let focus = state.diagnosis.focus();
            let mut rows = timeout_evidence(spec);
            rows.retain(|row| focus.is_none_or(|capability| row.capability == capability));
            rows
        }
    };
    if let Some(ttl) = spec.ttl {
        let ttl_ms = ttl.as_millis() as u64;
        for item in &mut evidence {
            if item.expires_at.is_none() && item.outcome != Outcome::Disabled {
                item.expires_at = Some(item.observed_at + ttl_ms);
            }
        }
    }
    (spec, evidence)
}

fn timeout_evidence(spec: &CheckSpec) -> Vec<Evidence> {
    let at = now_ms();
    spec.capabilities
        .iter()
        .map(|capability: &Capability| Evidence {
            source: format!("{}.deadline", spec.id),
            capability: *capability,
            route: spec.timeout_route.to_string(),
            tier: Tier::Static,
            // Memory only reaches LARM through an advisory service; a hung link must not block it.
            importance: if *capability == Capability::Memory && spec.timeout_route != "core" {
                super::contract::Importance::Advisory
            } else {
                super::contract::Importance::Required
            },
            outcome: Outcome::Unverified,
            reason: Reason::Timeout,
            subject: None,
            detail: None,
            latency_ms: Some(spec.timeout.as_millis() as u64),
            observed_at: at,
            expires_at: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::checks::{pass, CheckFuture};
    use crate::diagnosis::contract::CapabilityState;
    use rusqlite::Connection;

    fn fresh() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    fn quick_pass(_: &AppState) -> CheckFuture<'_> {
        Box::pin(async { vec![pass("fast", Capability::Storage, Tier::Static)] })
    }

    fn hangs(_: &AppState) -> CheckFuture<'_> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            Vec::new()
        })
    }

    fn spec(id: &'static str, run: for<'a> fn(&'a AppState) -> CheckFuture<'a>) -> CheckSpec {
        CheckSpec {
            id,
            quick: true,
            timeout: Duration::from_millis(50),
            ttl: Some(Duration::from_secs(60)),
            capabilities: &[Capability::Storage, Capability::Memory],
            timeout_route: "slow",
            run,
        }
    }

    #[tokio::test]
    async fn a_quick_scan_of_a_fresh_install_never_reports_ready_for_unproven_capabilities() {
        let state = fresh();
        let mut settings = state
            .sqlite_readers
            .read(crate::persistence::load_model_providers)
            .unwrap();
        settings.harness.address.clear();
        settings.providers.clear();
        let value = serde_json::to_string(&settings).unwrap();
        state
            .sqlite_writer
            .write(move |connection| {
                connection
                    .execute(
                        "UPDATE settings_documents SET value_json=?1
                         WHERE namespace='providers.model' AND key='default'",
                        [value],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .unwrap();
        let store = DiagnosisStore::new();
        store.try_begin();
        execute(&state, &store, &select(DiagnosisScope::Quick), |_| {}).await;
        store.finish();
        let report = store.snapshot();
        let of = |capability| {
            report
                .capabilities
                .iter()
                .find(|item| item.capability == capability)
                .unwrap()
                .state
        };
        assert_eq!(of(Capability::Storage), CapabilityState::Ready);
        assert_eq!(of(Capability::Conversation), CapabilityState::Unavailable);
        assert_eq!(of(Capability::VoiceListen), CapabilityState::Disabled);
        assert_ne!(report.overall, CapabilityState::Ready);
        assert_eq!(report.overall, CapabilityState::Unavailable);
    }

    #[tokio::test(start_paused = true)]
    async fn a_focused_timeout_only_marks_the_focused_capability() {
        let state = fresh();
        let store = &*state.diagnosis;
        store.try_begin();
        store.set_focus(Some(Capability::Memory));
        let slow: &'static CheckSpec = Box::leak(Box::new(spec("slow", hangs)));
        execute(&state, store, &[slow], |_| {}).await;
        store.finish();
        let report = store.snapshot();
        let rows = |capability| {
            report
                .capabilities
                .iter()
                .find(|item| item.capability == capability)
                .unwrap()
                .evidence
                .len()
        };
        assert_eq!(rows(Capability::Memory), 1);
        assert_eq!(rows(Capability::Storage), 0);
        let memory = report
            .capabilities
            .iter()
            .find(|item| item.capability == Capability::Memory)
            .unwrap();
        assert_eq!(
            memory.evidence[0].importance,
            super::super::contract::Importance::Advisory
        );
    }

    #[test]
    fn scopes_select_the_right_checks() {
        let quick = select(DiagnosisScope::Quick);
        assert!(quick.iter().all(|spec| spec.quick));
        let full = select(DiagnosisScope::Full);
        assert!(full.len() > quick.len());
        assert!(full.iter().any(|spec| spec.id == "harness.session"));
        assert!(quick.iter().all(|spec| spec.id != "harness.session"));
        let voice = select(DiagnosisScope::Capability {
            capability: Capability::VoiceSpeak,
        });
        assert!(voice.iter().any(|spec| spec.id == "providers.probe"));
        assert!(voice.iter().all(|spec| spec.id != "storage"));
    }

    #[test]
    fn quick_scope_never_contains_a_check_that_sends_provider_requests() {
        for spec in select(DiagnosisScope::Quick) {
            assert!(
                !matches!(spec.id, "providers.probe" | "codex.sdk" | "harness.session"),
                "{} must not run in a quick scan",
                spec.id
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_check_times_out_alone_and_ttl_is_stamped() {
        let state = fresh();
        let store = DiagnosisStore::new();
        store.try_begin();
        let ok: &'static CheckSpec = Box::leak(Box::new(spec("ok", quick_pass)));
        let slow: &'static CheckSpec = Box::leak(Box::new(spec("slow", hangs)));
        let mut progress = 0;
        execute(&state, &store, &[ok, slow], |_| progress += 1).await;
        store.finish();
        assert_eq!(progress, 2);
        let report = store.snapshot();
        let storage = report
            .capabilities
            .iter()
            .find(|item| item.capability == Capability::Storage)
            .unwrap();
        let fast = storage
            .evidence
            .iter()
            .find(|item| item.source == "fast")
            .unwrap();
        assert_eq!(fast.outcome, Outcome::Pass);
        assert!(fast.expires_at.is_some());
        let deadline = storage
            .evidence
            .iter()
            .find(|item| item.source == "slow.deadline")
            .unwrap();
        assert_eq!(
            (deadline.outcome, deadline.reason),
            (Outcome::Unverified, Reason::Timeout)
        );
        assert_eq!(deadline.route, "slow");
        // The timeout never turns a capability unavailable by itself, and never green either.
        assert_ne!(report.overall, CapabilityState::Ready);
    }
}
