use super::checks;
use super::contract::{DiagnosisItem, DiagnosisReport, DiagnosisSeverity, DiagnosisStatus};
use crate::AppState;
use tauri::{Emitter, Manager};

const GROUP_ORDER: [&str; 6] = ["storage", "settings", "llm", "voice", "harness", "memory"];

pub(crate) fn spawn_startup(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        run_and_publish(&app).await;
    });
}

pub(crate) async fn run_and_publish(app: &tauri::AppHandle) -> DiagnosisReport {
    let state = app.state::<AppState>();
    let store = std::sync::Arc::clone(&state.diagnosis);
    let Some(revision) = store.try_begin() else {
        store.wait_finished().await;
        return store.snapshot();
    };
    let started_at = crate::now_iso();
    let mut flight = InFlight::start(Some(app.clone()), store, revision, started_at);
    let mut items = Vec::new();
    stream(&state, |batch| {
        items.extend(batch);
        items.sort_by_key(|item| group_rank(&item.group));
        flight.progress(items.clone(), app);
    })
    .await;
    let report = flight.finish(items);
    let _ = app.emit("diagnosis-updated", &report);
    eprintln!(
        "self diagnosis published revision={} overall={}",
        report.revision,
        report.overall.as_str()
    );
    report
}

struct InFlight {
    app: Option<tauri::AppHandle>,
    store: std::sync::Arc<super::store::DiagnosisStore>,
    revision: u64,
    started_at: String,
    finished: bool,
}

impl InFlight {
    fn start(
        app: Option<tauri::AppHandle>,
        store: std::sync::Arc<super::store::DiagnosisStore>,
        revision: u64,
        started_at: String,
    ) -> Self {
        Self {
            app,
            store,
            revision,
            started_at,
            finished: false,
        }
    }

    fn progress(&mut self, items: Vec<DiagnosisItem>, app: &tauri::AppHandle) {
        let mut report = report_from(self.revision, self.started_at.clone(), items);
        report.running = true;
        report.finished_at = None;
        report.overall = DiagnosisStatus::Running;
        self.store.stage(report.clone());
        let _ = app.emit("diagnosis-updated", &report);
    }

    fn finish(&mut self, items: Vec<DiagnosisItem>) -> DiagnosisReport {
        let report = report_from(self.revision, self.started_at.clone(), items);
        self.store.publish(report.clone());
        self.finished = true;
        report
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let report = report_from(
            self.revision,
            self.started_at.clone(),
            vec![checks::item(
                "diagnosis.interrupted",
                "storage",
                "Diagnosis interrupted",
                DiagnosisStatus::Fail,
                DiagnosisSeverity::Degraded,
                "診断が中断されました。再診断してください。",
                None,
            )],
        );
        self.store.publish(report.clone());
        if let Some(app) = &self.app {
            let _ = app.emit("diagnosis-updated", &report);
            eprintln!(
                "self diagnosis published revision={} overall={}",
                report.revision,
                report.overall.as_str()
            );
        }
    }
}

fn report_from(revision: u64, started_at: String, items: Vec<DiagnosisItem>) -> DiagnosisReport {
    DiagnosisReport {
        revision,
        started_at,
        finished_at: Some(crate::now_iso()),
        running: false,
        overall: overall(&items),
        items,
    }
}

pub(crate) async fn collect(state: &AppState) -> Vec<DiagnosisItem> {
    let mut items = Vec::new();
    stream(state, |batch| items.extend(batch)).await;
    items.sort_by_key(|item| group_rank(&item.group));
    items
}

async fn stream(state: &AppState, mut on_batch: impl FnMut(Vec<DiagnosisItem>)) {
    let enabled = checks::providers::enabled_providers(state);
    let mut pending: Vec<
        std::pin::Pin<Box<dyn std::future::Future<Output = Vec<DiagnosisItem>> + Send + '_>>,
    > = vec![
        Box::pin(async { vec![checks::sqlite::sqlite(state)] }),
        Box::pin(async { vec![checks::settings::settings(state)] }),
        Box::pin(async { checks::harness::harness(state).await }),
        Box::pin(async { checks::memory::memory(state) }),
    ];
    for provider in &enabled {
        let provider = provider.clone();
        pending.push(Box::pin(async move {
            checks::providers::probe_one(state, &provider).await
        }));
    }
    pending.push(Box::pin(async {
        checks::providers::codex_item(state)
            .await
            .into_iter()
            .collect()
    }));
    while !pending.is_empty() {
        let (batch, _index, rest) = futures_util::future::select_all(pending).await;
        pending = rest;
        if !batch.is_empty() {
            on_batch(batch);
        }
    }
}

pub(crate) fn overall(items: &[DiagnosisItem]) -> DiagnosisStatus {
    if items.iter().any(|item| {
        item.status == DiagnosisStatus::Fail && item.severity == DiagnosisSeverity::Fatal
    }) {
        DiagnosisStatus::Fail
    } else if items
        .iter()
        .any(|item| item.status == DiagnosisStatus::Fail || item.status == DiagnosisStatus::Warn)
    {
        DiagnosisStatus::Warn
    } else {
        DiagnosisStatus::Ok
    }
}

fn group_rank(group: &str) -> usize {
    GROUP_ORDER
        .iter()
        .position(|candidate| *candidate == group)
        .unwrap_or(GROUP_ORDER.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};
    use crate::ModelProviderSettings;
    use rusqlite::Connection;
    use std::time::{Duration, Instant};

    fn sample(status: DiagnosisStatus, severity: DiagnosisSeverity) -> DiagnosisItem {
        DiagnosisItem {
            id: "sample".into(),
            group: "llm".into(),
            label: "Sample".into(),
            status,
            severity,
            message: String::new(),
            latency_ms: None,
        }
    }

    fn fresh() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    fn quiet_external(state: &AppState) {
        let mut settings = state
            .sqlite_readers
            .read(crate::persistence::load_model_providers)
            .expect("providers load");
        settings.harness.address.clear();
        settings
            .providers
            .retain(|provider| matches!(provider, ModelProviderSettings::SystemTts(_)));
        let mut provider = crate::test_support::direct_provider("local-llm", "local");
        provider.endpoint = "http://127.0.0.1:9/v1".to_string();
        settings
            .providers
            .push(ModelProviderSettings::OpenAiCompatible(provider));
        let value = serde_json::to_string(&settings).expect("settings encode");
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE settings_documents SET value_json=?1
                         WHERE namespace='providers.model' AND key='default'",
                        [value],
                    )
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("settings update");
    }

    #[test]
    fn dg_08_overall_fail_only_when_fatal_fails() {
        assert_eq!(
            overall(&[sample(DiagnosisStatus::Fail, DiagnosisSeverity::Degraded)]),
            DiagnosisStatus::Warn
        );
        assert_eq!(
            overall(&[sample(DiagnosisStatus::Warn, DiagnosisSeverity::Fatal)]),
            DiagnosisStatus::Warn
        );
        assert_eq!(
            overall(&[sample(DiagnosisStatus::Fail, DiagnosisSeverity::Fatal)]),
            DiagnosisStatus::Fail
        );
        assert_eq!(
            overall(&[sample(DiagnosisStatus::Ok, DiagnosisSeverity::Fatal)]),
            DiagnosisStatus::Ok
        );
    }

    #[test]
    fn dg_08_overall_warn_when_degraded_fails() {
        assert_eq!(
            overall(&[sample(DiagnosisStatus::Fail, DiagnosisSeverity::Degraded)]),
            DiagnosisStatus::Warn
        );
    }

    #[tokio::test]
    async fn dg_08_collect_orders_groups() {
        let state = fresh();
        quiet_external(&state);
        let items = collect(&state).await;
        let ranks = items
            .iter()
            .map(|item| group_rank(&item.group))
            .collect::<Vec<_>>();
        assert!(ranks.windows(2).all(|pair| pair[0] <= pair[1]));
        let mut groups = items
            .iter()
            .map(|item| item.group.as_str())
            .collect::<Vec<_>>();
        groups.dedup();
        assert_eq!(
            groups,
            ["storage", "settings", "llm", "voice", "harness", "memory"]
        );
    }

    #[tokio::test]
    async fn dg_08_collect_completes_on_fresh_state() {
        let state = fresh();
        quiet_external(&state);
        let started = Instant::now();
        let items = collect(&state).await;
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(!items.is_empty());
    }

    #[test]
    fn interrupted_run_unlocks_with_a_warning() {
        let store = std::sync::Arc::new(crate::diagnosis::store::DiagnosisStore::new());
        let revision = store.try_begin().expect("run starts");
        drop(InFlight::start(
            None,
            std::sync::Arc::clone(&store),
            revision,
            "t".into(),
        ));
        let snapshot = store.snapshot();
        assert!(!snapshot.running);
        assert_eq!(snapshot.revision, revision);
        assert_eq!(snapshot.overall, DiagnosisStatus::Warn);
        assert_eq!(snapshot.items[0].id, "diagnosis.interrupted");
        assert_eq!(store.try_begin(), Some(revision + 1));
    }
}
