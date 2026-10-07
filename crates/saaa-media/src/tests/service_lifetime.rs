use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use rusqlite::Connection;
use saaa_larm_session::media::{FailureKind, MediaArtifact, MediaError, MediaKind, MediaResult};
use saaa_provider_routing::{
    AdapterKind, BindingReview, Capability, CredentialRef, LarmReachability, Purpose,
    PurposeBinding, RegistrySnapshot, ServiceConnection, ServiceResource,
    PROVIDER_CREDENTIAL_SERVICE,
};
use tokio::sync::oneshot;

use crate::{
    initialize, BoxFut, Clock, DbOwner, FixedAvailability, GenerateCall, GenerateInput,
    HistoryQuery, ManualClock, MediaBackend, MediaService, RunTerminal, SqlStore,
};

fn snapshot(with_secret: bool) -> RegistrySnapshot {
    RegistrySnapshot {
        connections: vec![ServiceConnection {
            connection_id: "conn:harness".into(),
            label: "LARM".into(),
            adapter_kind: AdapterKind::Larm,
            endpoint: "http://127.0.0.1:9/".into(),
            location: "local".into(),
            authentication: if with_secret { "api-key" } else { "none" }.into(),
            credential_ref: with_secret.then(|| CredentialRef {
                service: PROVIDER_CREDENTIAL_SERVICE.into(),
                account: "missing".into(),
            }),
            enabled: true,
        }],
        resources: vec![ServiceResource {
            resource_id: "res:harness-image".into(),
            connection_id: "conn:harness".into(),
            capability: Capability::ImageGeneration,
            model: String::new(),
            detail: None,
            request_options: None,
            enabled: true,
        }],
        bindings: vec![PurposeBinding {
            purpose: Purpose::MediaImageGenerate,
            enabled: true,
            primary_resource_id: Some("res:harness-image".into()),
            fallback_resource_ids: Vec::new(),
            cloud_allowed: false,
            timeout_ms: 1_000,
            attempt_timeout_ms: Some(1_000),
            stored_primary_resource_id: None,
            review: BindingReview::Ready,
        }],
    }
}

struct SharedDb(Arc<Mutex<Connection>>);

impl DbOwner for SharedDb {
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

struct Gate {
    calls: Arc<AtomicUsize>,
    entered: Arc<AtomicBool>,
    hold: Mutex<Option<oneshot::Receiver<()>>>,
    burst: usize,
}

impl Gate {
    fn open(burst: usize) -> (Self, oneshot::Sender<()>, Arc<AtomicUsize>, Arc<AtomicBool>) {
        let (sender, receiver) = oneshot::channel();
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(AtomicBool::new(false));
        (
            Self {
                calls: calls.clone(),
                entered: entered.clone(),
                hold: Mutex::new(Some(receiver)),
                burst,
            },
            sender,
            calls,
            entered,
        )
    }
}

fn ok_result() -> MediaResult {
    MediaResult {
        kind: MediaKind::Image,
        model: "fixture".into(),
        job_id: None,
        artifacts: Vec::new(),
    }
}

impl MediaBackend for Gate {
    fn generate(&self, call: GenerateCall) -> BoxFut<Result<MediaResult, MediaError>> {
        self.entered.store(true, Ordering::SeqCst);
        let calls = self.calls.clone();
        let burst = self.burst;
        let progress = call.progress.clone();
        let receiver = self.hold.lock().ok().and_then(|mut slot| slot.take());
        Box::pin(async move {
            if let Some(receiver) = receiver {
                let _ = receiver.await;
            }
            for _ in 0..burst {
                progress(saaa_larm_session::media::MediaProgress {
                    phase: "generating".into(),
                    job_id: None,
                    progress: Some(0.1),
                });
            }
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(ok_result())
        })
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
        Box::pin(async {
            MediaError {
                kind: FailureKind::Cancelled,
                code: "idle".into(),
                retryable: false,
                may_have_generated: false,
                job_id: None,
            }
        })
    }

    fn fetch_artifact(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        _artifact: MediaArtifact,
    ) -> BoxFut<Result<Vec<u8>, MediaError>> {
        Box::pin(async { Err(idle_error()) })
    }
}

fn idle_error() -> MediaError {
    MediaError {
        kind: FailureKind::Cancelled,
        code: "idle".into(),
        retryable: false,
        may_have_generated: false,
        job_id: None,
    }
}

fn open_service(
    backend: Arc<dyn MediaBackend>,
    clock: Arc<dyn Clock>,
    with_secret: bool,
) -> (MediaService, Arc<Mutex<Connection>>) {
    let connection = Connection::open_in_memory().unwrap();
    saaa_provider_routing::initialize_settings_documents(&connection).unwrap();
    saaa_provider_routing::initialize_audit(&connection).unwrap();
    initialize(&connection).unwrap();
    let shared = Arc::new(Mutex::new(connection));
    let snapshot = snapshot(with_secret);
    let service = MediaService::new(
        Arc::new(SqlStore::new(
            SharedDb(shared.clone()),
            Arc::new(move |_| Ok(snapshot.clone())),
        )),
        Arc::new(FixedAvailability(LarmReachability::Reachable)),
        clock,
        backend,
    );
    (service, shared)
}

fn input(run: &str) -> GenerateInput {
    GenerateInput {
        run_id: run.into(),
        kind: MediaKind::Image,
        prompt: "fixture".into(),
    }
}

async fn until_entered(flag: &AtomicBool) {
    while !flag.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
}

fn status(shared: &Mutex<Connection>, run: &str) -> String {
    let connection = shared.lock().unwrap();
    crate::get(&connection, run).unwrap().unwrap()["status"]
        .as_str()
        .unwrap()
        .to_string()
}

const RUN: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1";

#[tokio::test]
async fn dropping_the_handle_does_not_cancel_or_resend() {
    let (gate, release, calls, entered) = Gate::open(0);
    let (service, shared) = open_service(Arc::new(gate), Arc::new(ManualClock::new("1")), false);
    let handle = service.submit(input(RUN)).await.unwrap();
    until_entered(&entered).await;
    drop(handle);
    release.send(()).unwrap();
    let mut follow = service.history(&HistoryQuery::ByRunId(RUN.into())).unwrap();
    for _ in 0..50 {
        if status(&shared, RUN) == "accepted" {
            break;
        }
        tokio::task::yield_now().await;
        follow = service.history(&HistoryQuery::ByRunId(RUN.into())).unwrap();
    }
    assert_eq!(status(&shared, RUN), "accepted");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(follow.len(), 1);
    let Err(again) = service.submit(input(RUN)).await else {
        panic!("a finished run must not be submitted again");
    };
    assert_eq!(again.code, "duplicate_run");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_saturated_progress_queue_still_reaches_one_terminal() {
    let (gate, release, calls, entered) = Gate::open(80);
    let (service, shared) = open_service(Arc::new(gate), Arc::new(ManualClock::new("1")), false);
    let mut handle = service.submit(input(RUN)).await.unwrap();
    until_entered(&entered).await;
    release.send(()).unwrap();
    let terminal = handle.wait_terminal().await;
    let RunTerminal::Output(output) = terminal else {
        panic!("saturated progress must still publish the stored terminal");
    };
    assert!(output.result.is_some());
    assert!(output.error.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(status(&shared, RUN), "accepted");
}

#[tokio::test]
async fn the_same_run_id_submitted_concurrently_sends_once() {
    let (gate, release, calls, entered) = Gate::open(0);
    let (service, _shared) = open_service(Arc::new(gate), Arc::new(ManualClock::new("1")), false);
    let service = Arc::new(service);
    let left = service.clone();
    let right = service.clone();
    let first = tokio::spawn(async move { left.submit(input(RUN)).await });
    let second = tokio::spawn(async move { right.submit(input(RUN)).await });
    until_entered(&entered).await;
    release.send(()).unwrap();
    for _ in 0..100 {
        if calls.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    let first = first.await.unwrap();
    let second = second.await.unwrap();
    let oks = match (&first, &second) {
        (Ok(_), Err(_)) | (Err(_), Ok(_)) => 1,
        (Ok(_), Ok(_)) => 2,
        (Err(left), Err(right)) => panic!("{} / {}", left.code, right.code),
    };
    assert_eq!(oks, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reserve_failure_does_not_send_and_releases_the_slot() {
    let (gate, _release, calls, entered) = Gate::open(0);
    let (service, shared) = open_service(Arc::new(gate), Arc::new(ManualClock::new("1")), true);
    let Err(error) = service.submit(input(RUN)).await else {
        panic!("a missing credential must fail before send");
    };
    assert_eq!(error.code, "storage_failed");
    assert!(!entered.load(Ordering::SeqCst));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(crate::get(&shared.lock().unwrap(), RUN).unwrap().is_none());
    let other = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2";
    let Err(error) = service.submit(input(other)).await else {
        panic!("a released slot must still reject the missing credential");
    };
    assert_eq!(error.code, "storage_failed");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn attempt_record_failure_releases_the_unsent_registration() {
    let (gate, _release, calls, entered) = Gate::open(0);
    let (service, shared) = open_service(Arc::new(gate), Arc::new(ManualClock::new("1")), false);
    shared
        .lock()
        .unwrap()
        .execute("DROP TABLE audit_events", [])
        .unwrap();
    let Err(error) = service.submit(input(RUN)).await else {
        panic!("a failed attempt record must not send");
    };
    assert_eq!(error.code, "storage_failed");
    assert!(!entered.load(Ordering::SeqCst));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(crate::get(&shared.lock().unwrap(), RUN).unwrap().is_none());
}

#[tokio::test]
async fn a_deadline_is_unknown_and_does_not_send_or_succeed() {
    let clock = Arc::new(ManualClock::new("1"));
    clock.expire_immediately();
    let (gate, _release, calls, _entered) = Gate::open(0);
    let (service, shared) = open_service(Arc::new(gate), clock, false);
    let mut handle = service.submit(input(RUN)).await.unwrap();
    let RunTerminal::Output(output) = handle.wait_terminal().await else {
        panic!("deadline must follow the stored outcome");
    };
    let error = output.error.expect("unknown");
    assert_eq!(error.code, "generation_deadline_exceeded");
    assert!(error.may_have_generated);
    assert_eq!(status(&shared, RUN), "unknown");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(service.submit(input(RUN)).await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
