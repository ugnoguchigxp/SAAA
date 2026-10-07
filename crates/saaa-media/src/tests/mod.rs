mod replicate;
mod service_commit;
mod service_lifetime;
mod shutdown;

use rusqlite::Connection;
use saaa_larm_session::media::{FailureKind, MediaError, MediaKind, MediaResult};
use saaa_provider_routing::{
    accepted, validate_active, AdapterKind, BindingReview, Capability, Purpose, PurposeBinding,
    RegistrySnapshot, RouteSelection, ServiceConnection, ServiceResource,
};

use crate::{cache, finish, get, initialize, reserve};

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

fn open() -> (
    Connection,
    RegistrySnapshot,
    saaa_provider_routing::ResolvedRoute,
) {
    let snapshot = image_snapshot();
    let route = saaa_provider_routing::resolve_route(
        &snapshot,
        Purpose::MediaImageGenerate,
        saaa_provider_routing::LarmReachability::Reachable,
    )
    .expect("route");
    assert_eq!(route.selection, RouteSelection::Primary);
    let connection = Connection::open_in_memory().unwrap();
    saaa_provider_routing::initialize_settings_documents(&connection).unwrap();
    saaa_provider_routing::initialize_audit(&connection).unwrap();
    initialize(&connection).unwrap();
    (connection, snapshot, route)
}

#[test]
fn duplicate_reserve_rejects_and_a_rolled_back_reserve_can_be_repeated() {
    let (mut connection, snapshot, route) = open();
    let run = "11111111-1111-1111-1111-111111111111";
    let tx = connection.transaction().unwrap();
    reserve(&tx, "1", run, &MediaKind::Image, &route, |db| {
        validate_active(db, &route, &snapshot)
    })
    .unwrap();
    tx.commit().unwrap();
    let tx = connection.transaction().unwrap();
    let error = reserve(&tx, "2", run, &MediaKind::Image, &route, |db| {
        validate_active(db, &route, &snapshot)
    })
    .unwrap_err();
    assert_eq!(
        error,
        "この生成要求は記録済みです。再送せず、進行状況を確認してください。"
    );
    drop(tx);
    let other = "22222222-2222-2222-2222-222222222222";
    let tx = connection.transaction().unwrap();
    reserve(&tx, "3", other, &MediaKind::Image, &route, |db| {
        validate_active(db, &route, &snapshot)
    })
    .unwrap();
    tx.rollback().unwrap();
    let tx = connection.transaction().unwrap();
    reserve(&tx, "4", other, &MediaKind::Image, &route, |db| {
        validate_active(db, &route, &snapshot)
    })
    .unwrap();
    tx.commit().unwrap();
    assert_eq!(
        get(&connection, other).unwrap().unwrap()["status"],
        "reserved"
    );
}

#[test]
fn audit_insert_failure_does_not_leave_an_accepted_row() {
    let (mut connection, snapshot, route) = open();
    let run = "33333333-3333-3333-3333-333333333333";
    {
        let tx = connection.transaction().unwrap();
        reserve(&tx, "1", run, &MediaKind::Image, &route, |db| {
            validate_active(db, &route, &snapshot)
        })
        .unwrap();
        tx.commit().unwrap();
    }
    connection.execute("DROP TABLE audit_events", []).unwrap();
    let tx = connection.transaction().unwrap();
    let result = Ok(MediaResult {
        kind: MediaKind::Image,
        model: "fixture".into(),
        job_id: None,
        artifacts: Vec::new(),
    });
    assert!(finish(
        &tx,
        "2",
        run,
        &route,
        &result,
        |db| validate_active(db, &route, &snapshot),
        |db, used| accepted(db, run, used, "2"),
    )
    .is_err());
    drop(tx);
    assert_eq!(
        get(&connection, run).unwrap().unwrap()["status"],
        "reserved"
    );
}

#[test]
fn cancelled_finish_records_cancelled_without_an_audit_acceptance() {
    let (mut connection, snapshot, route) = open();
    let run = "55555555-5555-5555-5555-555555555555";
    let tx = connection.transaction().unwrap();
    reserve(&tx, "1", run, &MediaKind::Image, &route, |db| {
        validate_active(db, &route, &snapshot)
    })
    .unwrap();
    tx.commit().unwrap();
    let error = MediaError {
        kind: FailureKind::Cancelled,
        code: "cancelled_before_submission".into(),
        retryable: false,
        may_have_generated: false,
        job_id: None,
    };
    let tx = connection.transaction().unwrap();
    finish(
        &tx,
        "3",
        run,
        &route,
        &Err(error),
        |db| validate_active(db, &route, &snapshot),
        |db, used| accepted(db, run, used, "3"),
    )
    .unwrap();
    tx.commit().unwrap();
    assert_eq!(
        get(&connection, run).unwrap().unwrap()["status"],
        "cancelled"
    );
}

#[test]
fn artifact_cache_rejects_oversize_bytes() {
    let error = cache(
        &Connection::open_in_memory().unwrap(),
        "run",
        0,
        &vec![0; 64 * 1024 * 1024 + 1],
    )
    .unwrap_err();
    assert_eq!(error, "成果物が大きすぎます");
}

#[test]
fn artifact_urls_allow_delivery_and_loopback_fixtures_only() {
    assert!(crate::replicate::allowed_artifact_url(
        "http://127.0.0.1:9/v1",
        "http://127.0.0.1:9/output.png",
    )
    .is_ok());
    assert!(crate::replicate::allowed_artifact_url(
        "http://[::1]:9/v1",
        "http://[::1]:9/output.png",
    )
    .is_ok());
    assert!(crate::replicate::allowed_artifact_url(
        "https://api.replicate.com/v1",
        "https://replicate.delivery/output.png",
    )
    .is_ok());
    assert!(crate::replicate::allowed_artifact_url(
        "https://api.replicate.com/v1",
        "https://pb.replicate.delivery/output.png",
    )
    .is_ok());
    for (endpoint, raw) in [
        ("http://127.0.0.1:9/v1", "http://[::1]:9/output.png"),
        ("http://[::1]:9/v1", "http://127.0.0.1:9/output.png"),
        (
            "https://api.replicate.com/v1",
            "https://example.com/output.png",
        ),
        (
            "https://api.replicate.com/v1",
            "https://replicate.delivery.example/output.png",
        ),
        (
            "https://api.replicate.com/v1",
            "https://user:secret@replicate.delivery/output.png",
        ),
        ("http://localhost:9/v1", "http://localhost:9/output.png"),
    ] {
        assert!(
            crate::replicate::allowed_artifact_url(endpoint, raw).is_err(),
            "{raw}"
        );
    }
}

#[test]
fn two_services_do_not_share_cancellation() {
    let left = harness_service();
    let right = harness_service();
    let id = "44444444-4444-4444-4444-444444444444";
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let outcome = left.cancel(id).await.unwrap();
        assert_eq!(outcome.status, crate::CancelStatus::Accepted);
        assert!(right
            .history(&crate::HistoryQuery::ByRunId(id.into()))
            .unwrap()
            .is_empty());
        assert!(left.cancel("not-a-uuid").await.is_err());
    });
}

#[test]
fn reconcile_of_an_interrupted_image_does_not_call_the_provider() {
    let (mut connection, snapshot, route) = open();
    let run = "55555555-5555-4555-8555-555555555555";
    let tx = connection.transaction().unwrap();
    reserve(&tx, "1", run, &MediaKind::Image, &route, |_| Ok(())).unwrap();
    tx.commit().unwrap();
    crate::reconcile_interrupted(&connection, "2").unwrap();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let service = service_with(connection, snapshot, calls.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut handle = service.reconcile(run).await.unwrap();
        let crate::RunTerminal::Output(output) = handle.wait_terminal().await else {
            panic!("interrupted reconcile must keep the stored unknown result");
        };
        assert_eq!(output.error.expect("unknown").code, "reconcile_interrupted");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(service
            .history(&crate::HistoryQuery::ByRunId("not-a-uuid".into()))
            .is_err());
    });
}

#[test]
fn explicit_reconcile_of_an_interrupted_remote_job_queries_the_provider() {
    recover_remote_job(remote_case(
        MediaKind::Music,
        AdapterKind::Larm,
        "local",
        false,
        Capability::MusicGeneration,
        Purpose::MediaMusicGenerate,
    ));
    recover_remote_job(remote_case(
        MediaKind::Image,
        AdapterKind::ReplicateMedia,
        "cloud",
        true,
        Capability::ImageGeneration,
        Purpose::MediaImageGenerate,
    ));
}

fn remote_case(
    kind: MediaKind,
    adapter: AdapterKind,
    location: &str,
    cloud_allowed: bool,
    capability: Capability,
    purpose: Purpose,
) -> (
    MediaKind,
    RegistrySnapshot,
    saaa_provider_routing::ResolvedRoute,
) {
    let snapshot = RegistrySnapshot {
        connections: vec![ServiceConnection {
            connection_id: "conn:harness".into(),
            label: "remote".into(),
            adapter_kind: adapter,
            endpoint: "http://127.0.0.1:9/".into(),
            location: location.into(),
            authentication: "none".into(),
            credential_ref: None,
            enabled: true,
        }],
        resources: vec![ServiceResource {
            resource_id: "res:harness".into(),
            connection_id: "conn:harness".into(),
            capability,
            model: "fixture".into(),
            detail: None,
            request_options: None,
            enabled: true,
        }],
        bindings: vec![PurposeBinding {
            purpose,
            enabled: true,
            primary_resource_id: Some("res:harness".into()),
            fallback_resource_ids: Vec::new(),
            cloud_allowed,
            timeout_ms: 1_000,
            attempt_timeout_ms: Some(1_000),
            stored_primary_resource_id: None,
            review: BindingReview::Ready,
        }],
    };
    let route = saaa_provider_routing::resolve_route(
        &snapshot,
        purpose,
        saaa_provider_routing::LarmReachability::Reachable,
    )
    .expect("route");
    (kind, snapshot, route)
}

fn recover_remote_job(
    case: (
        MediaKind,
        RegistrySnapshot,
        saaa_provider_routing::ResolvedRoute,
    ),
) {
    let (kind, snapshot, route) = case;
    let mut connection = Connection::open_in_memory().unwrap();
    saaa_provider_routing::initialize_settings_documents(&connection).unwrap();
    saaa_provider_routing::initialize_audit(&connection).unwrap();
    initialize(&connection).unwrap();
    let run = "66666666-6666-4666-8666-666666666666";
    let tx = connection.transaction().unwrap();
    reserve(&tx, "1", run, &kind, &route, |_| Ok(())).unwrap();
    tx.commit().unwrap();
    crate::phase(&connection, "1", run, "submitted", Some("job-1")).unwrap();
    crate::reconcile_interrupted(&connection, "2").unwrap();
    let tx = connection.transaction().unwrap();
    let outcome = finish(
        &tx,
        "3",
        run,
        &route,
        &Ok(MediaResult {
            kind,
            model: "late".into(),
            job_id: Some("job-1".into()),
            artifacts: Vec::new(),
        }),
        |_| Ok(()),
        |_, _| Ok(()),
    )
    .unwrap();
    assert!(matches!(outcome, crate::FinishOutcome::Unknown));
    tx.commit().unwrap();
    assert_eq!(get(&connection, run).unwrap().unwrap()["status"], "unknown");
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let service = crate::MediaService::new(
        std::sync::Arc::new(crate::SqlStore::new(
            crate::MutexDb::new(connection),
            std::sync::Arc::new(move |_| Ok(snapshot.clone())),
        )),
        std::sync::Arc::new(crate::FixedAvailability(
            saaa_provider_routing::LarmReachability::Reachable,
        )),
        std::sync::Arc::new(crate::ManualClock::new("4")),
        std::sync::Arc::new(QueryBackend {
            calls: calls.clone(),
            kind,
        }),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut handle = service.reconcile(run).await.unwrap();
        let crate::RunTerminal::Output(output) = handle.wait_terminal().await else {
            panic!("explicit reconcile must adopt the queried result");
        };
        assert!(output.error.is_none());
        assert_eq!(
            output.result.expect("result").job_id.as_deref(),
            Some("job-1")
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    });
}

struct QueryBackend {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    kind: MediaKind,
}

impl crate::MediaBackend for QueryBackend {
    fn generate(
        &self,
        _call: crate::GenerateCall,
    ) -> crate::contracts::BoxFut<Result<MediaResult, MediaError>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Err(idle_error()) })
    }
    fn reconcile(
        &self,
        _call: crate::GenerateCall,
    ) -> crate::contracts::BoxFut<Result<MediaResult, MediaError>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let kind = self.kind;
        Box::pin(async move {
            Ok(MediaResult {
                kind,
                model: "queried".into(),
                job_id: Some("job-1".into()),
                artifacts: Vec::new(),
            })
        })
    }
    fn cancel_remote(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        _job_id: String,
    ) -> crate::contracts::BoxFut<MediaError> {
        Box::pin(async { idle_error() })
    }
    fn fetch_artifact(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        _artifact: saaa_larm_session::media::MediaArtifact,
    ) -> crate::contracts::BoxFut<Result<Vec<u8>, MediaError>> {
        Box::pin(async { Err(idle_error()) })
    }
}

fn service_with(
    connection: Connection,
    snapshot: RegistrySnapshot,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) -> crate::MediaService {
    crate::MediaService::new(
        std::sync::Arc::new(crate::SqlStore::new(
            crate::MutexDb::new(connection),
            std::sync::Arc::new(move |_| Ok(snapshot.clone())),
        )),
        std::sync::Arc::new(crate::FixedAvailability(
            saaa_provider_routing::LarmReachability::Reachable,
        )),
        std::sync::Arc::new(crate::ManualClock::new("1")),
        std::sync::Arc::new(CountingBackend { calls }),
    )
}

struct CountingBackend {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl crate::MediaBackend for CountingBackend {
    fn generate(
        &self,
        _call: crate::GenerateCall,
    ) -> crate::contracts::BoxFut<Result<MediaResult, MediaError>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Err(idle_error()) })
    }
    fn reconcile(
        &self,
        _call: crate::GenerateCall,
    ) -> crate::contracts::BoxFut<Result<MediaResult, MediaError>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Err(idle_error()) })
    }
    fn cancel_remote(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        _job_id: String,
    ) -> crate::contracts::BoxFut<MediaError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { idle_error() })
    }
    fn fetch_artifact(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        _artifact: saaa_larm_session::media::MediaArtifact,
    ) -> crate::contracts::BoxFut<Result<Vec<u8>, MediaError>> {
        Box::pin(async { Err(idle_error()) })
    }
}

fn harness_service() -> crate::MediaService {
    let connection = Connection::open_in_memory().unwrap();
    saaa_provider_routing::initialize_settings_documents(&connection).unwrap();
    saaa_provider_routing::initialize_audit(&connection).unwrap();
    initialize(&connection).unwrap();
    let snapshot = image_snapshot();
    let store = std::sync::Arc::new(crate::SqlStore::new(
        crate::MutexDb::new(connection),
        std::sync::Arc::new(move |_db| Ok(snapshot.clone())),
    ));
    crate::MediaService::new(
        store,
        std::sync::Arc::new(crate::FixedAvailability(
            saaa_provider_routing::LarmReachability::Reachable,
        )),
        std::sync::Arc::new(crate::ManualClock::new("1")),
        std::sync::Arc::new(IdleBackend),
    )
}

struct IdleBackend;

impl crate::MediaBackend for IdleBackend {
    fn generate(
        &self,
        _call: crate::GenerateCall,
    ) -> crate::contracts::BoxFut<Result<saaa_larm_session::media::MediaResult, MediaError>> {
        Box::pin(async { Err(idle_error()) })
    }
    fn reconcile(
        &self,
        _call: crate::GenerateCall,
    ) -> crate::contracts::BoxFut<Result<saaa_larm_session::media::MediaResult, MediaError>> {
        Box::pin(async { Err(idle_error()) })
    }
    fn cancel_remote(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        _job_id: String,
    ) -> crate::contracts::BoxFut<MediaError> {
        Box::pin(async { idle_error() })
    }
    fn fetch_artifact(
        &self,
        _route: saaa_provider_routing::ResolvedRoute,
        _kind: MediaKind,
        _artifact: saaa_larm_session::media::MediaArtifact,
    ) -> crate::contracts::BoxFut<Result<Vec<u8>, MediaError>> {
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
