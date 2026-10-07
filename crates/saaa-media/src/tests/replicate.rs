use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use rusqlite::Connection;
use saaa_larm_session::media::{MediaKind, MediaProgress, MediaResult};
use saaa_provider_routing::{
    resolve_route, AdapterKind, BindingReview, Capability, LarmReachability, Purpose,
    PurposeBinding, RegistrySnapshot, ResolvedRoute, ServiceConnection, ServiceResource,
};
use tokio::sync::watch;

use crate::{cache, cached, finish, get, initialize, phase, replicate, reserve};

fn route_for(endpoint: &str) -> (Arc<Mutex<Connection>>, ResolvedRoute) {
    let snapshot = RegistrySnapshot {
        connections: vec![ServiceConnection {
            connection_id: "conn:svc-media-fixture".into(),
            label: "media fixture".into(),
            adapter_kind: AdapterKind::ReplicateMedia,
            endpoint: endpoint.into(),
            location: "cloud".into(),
            authentication: "none".into(),
            credential_ref: None,
            enabled: true,
        }],
        resources: vec![ServiceResource {
            resource_id: "res:svc-media-fixture".into(),
            connection_id: "conn:svc-media-fixture".into(),
            capability: Capability::ImageGeneration,
            model: "fixture/image".into(),
            detail: None,
            request_options: None,
            enabled: true,
        }],
        bindings: vec![PurposeBinding {
            purpose: Purpose::MediaImageGenerate,
            enabled: true,
            primary_resource_id: Some("res:svc-media-fixture".into()),
            fallback_resource_ids: Vec::new(),
            cloud_allowed: true,
            timeout_ms: 2000,
            attempt_timeout_ms: Some(1000),
            stored_primary_resource_id: None,
            review: BindingReview::Ready,
        }],
    };
    let route = resolve_route(
        &snapshot,
        Purpose::MediaImageGenerate,
        LarmReachability::Unknown,
    )
    .unwrap();
    let connection = Connection::open_in_memory().unwrap();
    saaa_provider_routing::initialize_settings_documents(&connection).unwrap();
    saaa_provider_routing::initialize_audit(&connection).unwrap();
    initialize(&connection).unwrap();
    (Arc::new(Mutex::new(connection)), route)
}

fn io(db: &Arc<Mutex<Connection>>, run: &str) -> replicate::ReplicateIo {
    let phase_db = db.clone();
    let cache_db = db.clone();
    let phase_run = run.to_string();
    let cache_run = run.to_string();
    replicate::ReplicateIo {
        validate: Arc::new(|_| Ok(())),
        load_secret: Arc::new(|_, _| Ok(None)),
        phase: Arc::new(move |name, job| {
            let connection = phase_db.lock().unwrap();
            phase(&connection, "1", &phase_run, name, job)
        }),
        cache: Arc::new(move |index, bytes| {
            let connection = cache_db.lock().unwrap();
            cache(&connection, &cache_run, index, bytes)
        }),
    }
}

fn reserve_run(db: &Mutex<Connection>, run: &str, route: &ResolvedRoute) {
    let mut connection = db.lock().unwrap();
    let tx = connection.transaction().unwrap();
    reserve(&tx, "1", run, &MediaKind::Image, route, |_| Ok(())).unwrap();
    tx.commit().unwrap();
}

#[tokio::test]
async fn durable_identity_resume_and_cache_never_repost_a_prediction() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let posts = Arc::new(AtomicUsize::new(0));
    let gets = Arc::new(AtomicUsize::new(0));
    let posted = posts.clone();
    let polled = gets.clone();
    let output = format!("http://{address}/output.png");
    let result = serde_json::json!({"id":"job_fixture","status":"succeeded","output":output});
    let created = result.clone();
    let fetched = result.clone();
    let router = axum::Router::new()
        .route(
            "/v1/models/fixture/image/predictions",
            axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let posted = posted.clone();
                let result = created.clone();
                async move {
                    posted.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(body["input"]["prompt"], "fixture prompt");
                    axum::Json(result)
                }
            }),
        )
        .route(
            "/v1/predictions/job_fixture",
            axum::routing::get(move || {
                let polled = polled.clone();
                let result = fetched.clone();
                async move {
                    polled.fetch_add(1, Ordering::SeqCst);
                    axum::Json(result)
                }
            }),
        )
        .route(
            "/output.png",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                assert!(!headers.contains_key("authorization"));
                ([("content-type", "image/png")], vec![1u8, 2, 3, 4])
            }),
        );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (db, route) = route_for(&format!("http://{address}/v1"));
    let run = "dddddddd-dddd-4ddd-8ddd-ddddddddddd1";
    reserve_run(&db, run, &route);
    let (_cancel, receiver) = watch::channel(false);
    let calls = io(&db, run);
    let progress = |_: MediaProgress| {};
    let generated = replicate::generate(
        &calls,
        run,
        MediaKind::Image,
        "fixture prompt",
        &route,
        receiver.clone(),
        &progress,
        None,
    )
    .await;
    assert!(generated.is_ok());
    assert!(cached(&db.lock().unwrap(), run, 0).unwrap().is_none());
    {
        let mut connection = db.lock().unwrap();
        let tx = connection.transaction().unwrap();
        assert!(reserve(&tx, "2", run, &MediaKind::Image, &route, |_| Ok(())).is_err());
    }
    let stored = get(&db.lock().unwrap(), run).unwrap().unwrap();
    assert_eq!(stored["jobId"], "job_fixture");
    let resumed = replicate::generate(
        &calls,
        run,
        MediaKind::Image,
        "",
        &route,
        receiver,
        &progress,
        stored["jobId"].as_str(),
    )
    .await;
    {
        let mut connection = db.lock().unwrap();
        let tx = connection.transaction().unwrap();
        finish(
            &tx,
            "3",
            run,
            &route,
            &resumed,
            |_| Ok(()),
            |database, used| saaa_provider_routing::accepted(database, run, used, "3"),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    assert_eq!(gets.load(Ordering::SeqCst), 1);
    assert_eq!(
        cached(&db.lock().unwrap(), run, 0).unwrap().unwrap(),
        [1, 2, 3, 4]
    );
    assert_eq!(
        get(&db.lock().unwrap(), run).unwrap().unwrap()["status"],
        "accepted"
    );
    server.abort();
}

#[tokio::test]
async fn unconfirmed_cancel_and_revocation_never_claim_success_or_adopt_staged_bytes() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = axum::Router::new().route(
        "/v1/predictions/job_fixture/cancel",
        axum::routing::post(|| async { axum::Json(serde_json::json!({"status":"processing"})) }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (db, route) = route_for(&format!("http://{address}/v1"));
    let run = "dddddddd-dddd-4ddd-8ddd-ddddddddddd2";
    reserve_run(&db, run, &route);
    phase(
        &db.lock().unwrap(),
        "1",
        run,
        "running",
        Some("job_fixture"),
    )
    .unwrap();
    let calls = io(&db, run);
    let error = replicate::cancel_existing(&calls, &route, "job_fixture").await;
    assert!(error.may_have_generated);
    assert_eq!(error.code, "remote_cancel_unconfirmed");
    {
        let mut connection = db.lock().unwrap();
        let tx = connection.transaction().unwrap();
        finish(
            &tx,
            "2",
            run,
            &route,
            &Err(error),
            |_| Ok(()),
            |database, used| saaa_provider_routing::accepted(database, run, used, "2"),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(
        get(&db.lock().unwrap(), run).unwrap().unwrap()["status"],
        "unknown"
    );
    cache(&db.lock().unwrap(), run, 0, &[1, 2]).unwrap();
    let mut revoked = route.clone();
    revoked.location = "cloud".into();
    let snapshot = RegistrySnapshot {
        connections: vec![],
        resources: vec![],
        bindings: vec![],
    };
    let result = Ok(MediaResult {
        kind: MediaKind::Image,
        model: route.model.clone(),
        job_id: Some("job_fixture".into()),
        artifacts: vec![],
    });
    {
        let mut connection = db.lock().unwrap();
        let tx = connection.transaction().unwrap();
        assert!(finish(
            &tx,
            "3",
            run,
            &revoked,
            &result,
            |database| saaa_provider_routing::validate_active(database, &revoked, &snapshot),
            |database, used| saaa_provider_routing::accepted(database, run, used, "3"),
        )
        .is_err());
    }
    assert_eq!(
        get(&db.lock().unwrap(), run).unwrap().unwrap()["status"],
        "unknown"
    );
    assert!(cached(&db.lock().unwrap(), run, 0).unwrap().is_none());
    server.abort();
}

#[tokio::test]
async fn artifact_redirect_and_unknown_mime_are_refused() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = axum::Router::new()
        .route(
            "/redirect",
            axum::routing::get(|| async {
                axum::response::Redirect::temporary("https://example.com/secret")
            }),
        )
        .route(
            "/unsafe",
            axum::routing::get(|| async { ([("content-type", "image/svg+xml")], "<svg/>") }),
        );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (_db, route) = route_for(&format!("http://{address}/v1"));
    for path in ["redirect", "unsafe"] {
        assert!(replicate::download(
            &route,
            &format!("http://{address}/{path}"),
            tokio::time::Instant::now() + std::time::Duration::from_secs(1)
        )
        .await
        .is_err());
    }
    assert!(replicate::download(
        &route,
        "https://example.com/file.png",
        tokio::time::Instant::now() + std::time::Duration::from_secs(1)
    )
    .await
    .is_err());
    server.abort();
}
