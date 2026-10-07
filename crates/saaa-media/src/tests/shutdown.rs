use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use rusqlite::Connection;
use saaa_larm_session::media::{FailureKind, MediaArtifact, MediaError, MediaKind, MediaResult};
use saaa_provider_routing::{
    AdapterKind, BindingReview, Capability, LarmReachability, Purpose, PurposeBinding,
    RegistrySnapshot, ServiceConnection, ServiceResource,
};

use crate::{
    initialize, BoxFut, DbOwner, FixedAvailability, GenerateCall, GenerateInput, ManualClock,
    MediaBackend, MediaService, RunTerminal, SqlStore,
};

fn replicate_snapshot() -> RegistrySnapshot {
    RegistrySnapshot {
        connections: vec![ServiceConnection {
            connection_id: "conn:media".into(),
            label: "media".into(),
            adapter_kind: AdapterKind::ReplicateMedia,
            endpoint: "http://127.0.0.1:9/v1".into(),
            location: "cloud".into(),
            authentication: "none".into(),
            credential_ref: None,
            enabled: true,
        }],
        resources: vec![ServiceResource {
            resource_id: "res:media".into(),
            connection_id: "conn:media".into(),
            capability: Capability::ImageGeneration,
            model: "owner/model".into(),
            detail: None,
            request_options: None,
            enabled: true,
        }],
        bindings: vec![PurposeBinding {
            purpose: Purpose::MediaImageGenerate,
            enabled: true,
            primary_resource_id: Some("res:media".into()),
            fallback_resource_ids: Vec::new(),
            cloud_allowed: true,
            timeout_ms: 60_000,
            attempt_timeout_ms: Some(60_000),
            stored_primary_resource_id: None,
            review: BindingReview::Ready,
        }],
    }
}

struct MemDb(Arc<Mutex<Connection>>);

impl DbOwner for MemDb {
    fn read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        let connection = self
            .0
            .lock()
            .map_err(|_| "Database writer unavailable".to_string())?;
        operation(&connection)
    }
    fn write<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        let mut connection = self
            .0
            .lock()
            .map_err(|_| "Database writer unavailable".to_string())?;
        operation(&mut connection)
    }
}

struct HoldBackend {
    calls: Arc<AtomicUsize>,
    entered: Arc<AtomicBool>,
    hold: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl MediaBackend for HoldBackend {
    fn generate(&self, _call: GenerateCall) -> BoxFut<Result<MediaResult, MediaError>> {
        self.entered.store(true, Ordering::SeqCst);
        let calls = self.calls.clone();
        let receiver = self.hold.lock().ok().and_then(|mut slot| slot.take());
        Box::pin(async move {
            if let Some(receiver) = receiver {
                let _ = receiver.await;
            }
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(MediaResult {
                kind: MediaKind::Image,
                model: "owner/model".into(),
                job_id: Some("job".into()),
                artifacts: Vec::new(),
            })
        })
    }
    fn reconcile(&self, call: GenerateCall) -> BoxFut<Result<MediaResult, MediaError>> {
        self.generate(call)
    }
    fn cancel_remote(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        job_id: String,
    ) -> BoxFut<MediaError> {
        Box::pin(async move {
            MediaError {
                kind: FailureKind::OutcomeUnknown,
                code: "remote_cancel_unconfirmed".into(),
                retryable: false,
                may_have_generated: true,
                job_id: Some(job_id),
            }
        })
    }
    fn fetch_artifact(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        _artifact: MediaArtifact,
    ) -> BoxFut<Result<Vec<u8>, MediaError>> {
        Box::pin(async {
            Err(MediaError {
                kind: FailureKind::ArtifactFailed,
                code: "idle".into(),
                retryable: false,
                may_have_generated: false,
                job_id: None,
            })
        })
    }
}

type Opened = (
    MediaService,
    Arc<Mutex<Connection>>,
    Arc<AtomicUsize>,
    Arc<AtomicBool>,
    tokio::sync::oneshot::Sender<()>,
);

fn open(clock: Arc<ManualClock>) -> Opened {
    let connection = Connection::open_in_memory().unwrap();
    saaa_provider_routing::initialize_settings_documents(&connection).unwrap();
    saaa_provider_routing::initialize_audit(&connection).unwrap();
    initialize(&connection).unwrap();
    let shared = Arc::new(Mutex::new(connection));
    let snapshot = replicate_snapshot();
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let service = MediaService::new(
        Arc::new(SqlStore::new(
            MemDb(shared.clone()),
            Arc::new(move |_| Ok(snapshot.clone())),
        )),
        Arc::new(FixedAvailability(LarmReachability::Reachable)),
        clock,
        Arc::new(HoldBackend {
            calls: calls.clone(),
            entered: entered.clone(),
            hold: Mutex::new(Some(receiver)),
        }),
    );
    (service, shared, calls, entered, sender)
}

fn input(run: &str) -> GenerateInput {
    GenerateInput {
        run_id: run.into(),
        kind: MediaKind::Image,
        prompt: "fixture".into(),
    }
}

fn status(shared: &Mutex<Connection>, run: &str) -> String {
    crate::get(&shared.lock().unwrap(), run).unwrap().unwrap()["status"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn until_entered(flag: &AtomicBool) {
    while !flag.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn shutdown_waits_for_an_in_flight_terminal_then_keeps_it() {
    let clock = Arc::new(ManualClock::new("1"));
    let (service, shared, calls, entered, release) = open(clock.clone());
    let run = "cccccccc-cccc-4ccc-8ccc-ccccccccccc1";
    let mut handle = service.submit(input(run)).await.unwrap();
    until_entered(&entered).await;
    let service = Arc::new(service);
    let shutting = service.clone();
    let shutdown =
        tokio::spawn(async move { shutting.shutdown(std::time::Duration::from_secs(10)).await });
    while status(&shared, run) != "cancel_requested" {
        tokio::task::yield_now().await;
    }
    release.send(()).unwrap();
    let RunTerminal::Output(output) = handle.wait_terminal().await else {
        panic!("shutdown must publish the stored terminal");
    };
    assert!(output.result.is_none());
    clock.release_sleepers();
    shutdown.await.unwrap().unwrap();
    assert_eq!(status(&shared, run), "cancelled");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(service.submit(input(run)).await.is_err());
}

#[tokio::test]
async fn grace_expiry_then_a_late_success_does_not_overwrite() {
    let clock = Arc::new(ManualClock::new("1"));
    let (service, shared, calls, entered, release) = open(clock.clone());
    let run = "cccccccc-cccc-4ccc-8ccc-ccccccccccc2";
    let _handle = service.submit(input(run)).await.unwrap();
    until_entered(&entered).await;
    clock.expire_immediately();
    service
        .shutdown(std::time::Duration::from_secs(10))
        .await
        .unwrap();
    release.send(()).unwrap();
    for _ in 0..100 {
        if calls.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(status(&shared, run), "unknown");
    let connection = shared.lock().unwrap();
    let audits: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM audit_events WHERE event_name='purpose-route-accepted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(audits, 0);
}

#[tokio::test]
async fn reopen_keeps_accepted_rows_and_marks_unconfirmed_without_sending() {
    let path =
        std::env::temp_dir().join(format!("saaa-media-reopen-{}.sqlite", uuid::Uuid::new_v4()));
    let run = "cccccccc-cccc-4ccc-8ccc-ccccccccccc3";
    let pending = "cccccccc-cccc-4ccc-8ccc-ccccccccccc4";
    {
        let connection = Connection::open(&path).unwrap();
        saaa_provider_routing::initialize_settings_documents(&connection).unwrap();
        saaa_provider_routing::initialize_audit(&connection).unwrap();
        initialize(&connection).unwrap();
        let snapshot = replicate_snapshot();
        let route = saaa_provider_routing::resolve_route(
            &snapshot,
            Purpose::MediaImageGenerate,
            LarmReachability::Reachable,
        )
        .unwrap();
        let mut connection = connection;
        let tx = connection.transaction().unwrap();
        crate::reserve(&tx, "1", run, &MediaKind::Image, &route, |_| Ok(())).unwrap();
        crate::reserve(&tx, "1", pending, &MediaKind::Image, &route, |_| Ok(())).unwrap();
        let result = Ok(MediaResult {
            kind: MediaKind::Image,
            model: "owner/model".into(),
            job_id: None,
            artifacts: Vec::new(),
        });
        crate::finish(
            &tx,
            "2",
            run,
            &route,
            &result,
            |_| Ok(()),
            |db, used| saaa_provider_routing::accepted(db, run, used, "2"),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let connection = Connection::open(&path).unwrap();
    let snapshot = replicate_snapshot();
    let service = MediaService::new(
        Arc::new(SqlStore::new(
            crate::MutexDb::new(connection),
            Arc::new(move |_| Ok(snapshot.clone())),
        )),
        Arc::new(FixedAvailability(LarmReachability::Reachable)),
        Arc::new(ManualClock::new("3")),
        Arc::new(Counting(counted)),
    );
    service.reconcile_interrupted().unwrap();
    assert_eq!(
        service
            .history(&crate::HistoryQuery::ByRunId(run.into()))
            .unwrap()[0]["status"],
        "accepted"
    );
    assert_eq!(
        service
            .history(&crate::HistoryQuery::ByRunId(pending.into()))
            .unwrap()[0]["status"],
        "unknown"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let _ = std::fs::remove_file(&path);
}

struct Counting(Arc<AtomicUsize>);

impl MediaBackend for Counting {
    fn generate(&self, _call: GenerateCall) -> BoxFut<Result<MediaResult, MediaError>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(idle()) })
    }
    fn reconcile(&self, call: GenerateCall) -> BoxFut<Result<MediaResult, MediaError>> {
        self.generate(call)
    }
    fn cancel_remote(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        _job_id: String,
    ) -> BoxFut<MediaError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { idle() })
    }
    fn fetch_artifact(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        _artifact: MediaArtifact,
    ) -> BoxFut<Result<Vec<u8>, MediaError>> {
        Box::pin(async { Err(idle()) })
    }
}

fn idle() -> MediaError {
    MediaError {
        kind: FailureKind::Cancelled,
        code: "idle".into(),
        retryable: false,
        may_have_generated: false,
        job_id: None,
    }
}
