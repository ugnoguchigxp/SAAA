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
use tokio::sync::oneshot;

use crate::{
    initialize, BoxFut, DbOwner, FixedAvailability, GenerateCall, GenerateInput, ManualClock,
    MediaBackend, MediaHostError, MediaService, RunTerminal, SqlStore,
};

fn image_snapshot() -> RegistrySnapshot {
    RegistrySnapshot {
        connections: vec![ServiceConnection {
            connection_id: "conn:harness".into(),
            label: "LARM".into(),
            adapter_kind: AdapterKind::Larm,
            endpoint: "http://127.0.0.1:9/".into(),
            location: "local".into(),
            authentication: "none".into(),
            credential_ref: None,
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

struct SharedDb {
    connection: Arc<Mutex<Connection>>,
    writes_after_arm: AtomicUsize,
    arm_drop: AtomicBool,
}

impl DbOwner for SharedDb {
    fn read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "Database writer unavailable".to_string())?;
        operation(&connection)
    }

    fn write<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        let armed = self.arm_drop.load(Ordering::SeqCst);
        let index = if armed {
            self.writes_after_arm.fetch_add(1, Ordering::SeqCst)
        } else {
            0
        };
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "Database writer unavailable".to_string())?;
        if armed && index == 1 {
            connection.execute("DROP TABLE audit_events", []).ok();
        }
        operation(&mut connection)
    }
}

struct Gate {
    entered: Arc<AtomicBool>,
    hold: Mutex<Option<oneshot::Receiver<()>>>,
}

impl Gate {
    fn open() -> (Self, oneshot::Sender<()>, Arc<AtomicBool>) {
        let (sender, receiver) = oneshot::channel();
        let entered = Arc::new(AtomicBool::new(false));
        (
            Self {
                entered: entered.clone(),
                hold: Mutex::new(Some(receiver)),
            },
            sender,
            entered,
        )
    }
}

impl MediaBackend for Gate {
    fn generate(&self, _call: GenerateCall) -> BoxFut<Result<MediaResult, MediaError>> {
        self.entered.store(true, Ordering::SeqCst);
        let receiver = self.hold.lock().ok().and_then(|mut slot| slot.take());
        Box::pin(async move {
            if let Some(receiver) = receiver {
                let _ = receiver.await;
            }
            Ok(MediaResult {
                kind: MediaKind::Image,
                model: "fixture".into(),
                job_id: None,
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
        _job_id: String,
    ) -> BoxFut<MediaError> {
        Box::pin(async {
            MediaError {
                kind: FailureKind::OutcomeUnknown,
                code: "remote_cancel_unconfirmed".into(),
                retryable: false,
                may_have_generated: true,
                job_id: Some("job".into()),
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

type RegistryLoader = dyn Fn(&Connection) -> Result<RegistrySnapshot, String> + Send + Sync;

fn service_for(
    backend: Arc<dyn MediaBackend>,
    registry: Arc<RegistryLoader>,
) -> (MediaService, Arc<SharedDb>) {
    let connection = Connection::open_in_memory().unwrap();
    saaa_provider_routing::initialize_settings_documents(&connection).unwrap();
    saaa_provider_routing::initialize_audit(&connection).unwrap();
    initialize(&connection).unwrap();
    let shared = Arc::new(SharedDb {
        connection: Arc::new(Mutex::new(connection)),
        writes_after_arm: AtomicUsize::new(0),
        arm_drop: AtomicBool::new(false),
    });
    let owner = SharedOwner(shared.clone());
    let service = MediaService::new(
        Arc::new(SqlStore::new(owner, registry)),
        Arc::new(FixedAvailability(LarmReachability::Reachable)),
        Arc::new(ManualClock::new("1")),
        backend,
    );
    (service, shared)
}

struct SharedOwner(Arc<SharedDb>);

impl DbOwner for SharedOwner {
    fn read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        self.0.read(operation)
    }
    fn write<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, String> + Send,
    ) -> Result<T, String> {
        self.0.write(operation)
    }
}

fn input() -> GenerateInput {
    GenerateInput {
        run_id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1".into(),
        kind: MediaKind::Image,
        prompt: "fixture".into(),
    }
}

async fn until_entered(flag: &AtomicBool) {
    while !flag.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
}

fn status(shared: &SharedDb) -> String {
    let connection = shared.connection.lock().unwrap();
    crate::get(&connection, "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1")
        .unwrap()
        .unwrap()["status"]
        .as_str()
        .unwrap()
        .to_string()
}

fn accepted_audits(shared: &SharedDb) -> i64 {
    let connection = shared.connection.lock().unwrap();
    connection
        .query_row(
            "SELECT COUNT(*) FROM audit_events WHERE event_name='purpose-route-accepted'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0)
}

#[tokio::test]
async fn cancel_committed_before_adoption_does_not_publish_success() {
    let (gate, release, entered) = Gate::open();
    let snapshot = image_snapshot();
    let (service, shared) = service_for(Arc::new(gate), Arc::new(move |_| Ok(snapshot.clone())));
    let mut handle = service.submit(input()).await.unwrap();
    until_entered(&entered).await;
    service
        .cancel("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1")
        .await
        .unwrap();
    release.send(()).unwrap();
    let RunTerminal::Output(output) = handle.wait_terminal().await else {
        panic!("cancel that wins must still be a stored terminal");
    };
    assert!(output.result.is_none());
    assert_eq!(output.error.unwrap().kind, FailureKind::Cancelled);
    assert_eq!(status(&shared), "cancelled");
    assert_eq!(accepted_audits(&shared), 0);
}

#[tokio::test]
async fn adoption_committed_before_cancel_is_not_rewritten() {
    let (gate, release, entered) = Gate::open();
    let snapshot = image_snapshot();
    let (service, shared) = service_for(Arc::new(gate), Arc::new(move |_| Ok(snapshot.clone())));
    let mut handle = service.submit(input()).await.unwrap();
    until_entered(&entered).await;
    release.send(()).unwrap();
    let RunTerminal::Output(output) = handle.wait_terminal().await else {
        panic!("adoption must publish the stored success");
    };
    assert!(output.result.is_some());
    assert_eq!(status(&shared), "accepted");
    let outcome = service
        .cancel("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1")
        .await
        .unwrap();
    assert_eq!(outcome.status, crate::CancelStatus::AlreadyTerminal);
    assert_eq!(status(&shared), "accepted");
    assert_eq!(accepted_audits(&shared), 1);
}

#[tokio::test]
async fn route_loss_during_adoption_rolls_back_without_a_success_terminal() {
    let (gate, release, entered) = Gate::open();
    let snapshot = Arc::new(Mutex::new(image_snapshot()));
    let registry_snapshot = snapshot.clone();
    let (service, shared) = service_for(
        Arc::new(gate),
        Arc::new(move |_| Ok(registry_snapshot.lock().unwrap().clone())),
    );
    let mut handle = service.submit(input()).await.unwrap();
    until_entered(&entered).await;
    snapshot.lock().unwrap().resources[0].enabled = false;
    release.send(()).unwrap();
    let terminal = handle.wait_terminal().await;
    assert!(matches!(terminal, RunTerminal::HostFailure(_)));
    assert_ne!(status(&shared), "accepted");
    assert_eq!(accepted_audits(&shared), 0);
}

#[tokio::test]
async fn audit_insert_failure_during_finish_rolls_back_the_adoption() {
    let (gate, release, entered) = Gate::open();
    let snapshot = image_snapshot();
    let (service, shared) = service_for(Arc::new(gate), Arc::new(move |_| Ok(snapshot.clone())));
    let mut handle = service.submit(input()).await.unwrap();
    until_entered(&entered).await;
    shared.arm_drop.store(true, Ordering::SeqCst);
    release.send(()).unwrap();
    let terminal = handle.wait_terminal().await;
    assert!(matches!(
        terminal,
        RunTerminal::HostFailure(MediaHostError { .. })
    ));
    assert_ne!(status(&shared), "accepted");
}
