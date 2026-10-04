use super::*;
use crate::providers::service_registry::{
    BindingReview, Capability, ServiceConnection, ServiceResource,
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn state(endpoint: &str) -> (AppState, ResolvedRoute) {
    let mut db = rusqlite::Connection::open_in_memory().unwrap();
    crate::initialize_database(&db).unwrap();
    let loaded = persistence::service_registry_store::load_registry(&db).unwrap();
    let mut snapshot = loaded.snapshot;
    snapshot.connections.push(ServiceConnection {
        connection_id: "conn:svc-media-fixture".into(),
        label: "media fixture".into(),
        adapter_kind: AdapterKind::ReplicateMedia,
        endpoint: endpoint.into(),
        location: "cloud".into(),
        authentication: "none".into(),
        credential_ref: None,
        enabled: true,
    });
    snapshot.resources.push(ServiceResource {
        resource_id: "res:svc-media-fixture".into(),
        connection_id: "conn:svc-media-fixture".into(),
        capability: Capability::ImageGeneration,
        model: "fixture/image".into(),
        detail: None,
        request_options: None,
        enabled: true,
    });
    let binding = snapshot
        .bindings
        .iter_mut()
        .find(|b| b.purpose == Purpose::MediaImageGenerate)
        .unwrap();
    binding.primary_resource_id = Some("res:svc-media-fixture".into());
    binding.cloud_allowed = true;
    binding.review = BindingReview::Ready;
    binding.timeout_ms = 2000;
    binding.attempt_timeout_ms = Some(1000);
    let saved =
        persistence::service_registry_store::save_registry(&mut db, &snapshot, loaded.revision)
            .unwrap();
    let route = crate::providers::service_registry::resolve_route(
        &saved.snapshot,
        Purpose::MediaImageGenerate,
    )
    .unwrap();
    (crate::test_support::app_state(db), route)
}

#[tokio::test]
async fn durable_identity_resume_and_cache_never_repost_a_prediction() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let posts = Arc::new(AtomicUsize::new(0));
    let gets = Arc::new(AtomicUsize::new(0));
    let p = posts.clone();
    let g = gets.clone();
    let output = format!("http://{address}/output.png");
    let result = json!({"id":"job_fixture","status":"succeeded","output":output});
    let posted = result.clone();
    let polled = result.clone();
    let router = axum::Router::new()
        .route(
            "/v1/models/fixture/image/predictions",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let p = p.clone();
                let result = posted.clone();
                async move {
                    p.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(body["input"]["prompt"], "fixture prompt");
                    axum::Json(result)
                }
            }),
        )
        .route(
            "/v1/predictions/job_fixture",
            axum::routing::get(move || {
                let g = g.clone();
                let result = polled.clone();
                async move {
                    g.fetch_add(1, Ordering::SeqCst);
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
    let (state, route) = state(&format!("http://{address}/v1"));
    let run = uuid::Uuid::new_v4().to_string();
    ledger::reserve(&state, &run, MediaKind::Image, &route).unwrap();
    let (_cancel, receiver) = watch::channel(false);
    let channel = tauri::ipc::Channel::new(|_| Ok(()));
    let result = replicate::generate(
        &state,
        &run,
        MediaKind::Image,
        "fixture prompt",
        &route,
        receiver.clone(),
        &channel,
        None,
    )
    .await;
    assert!(result.is_ok());
    assert!(ledger::cached(&state, &run, 0).unwrap().is_none());
    // Persisted identity survives loss of the in-memory registry. A repeated
    // request cannot be reserved; recovery issues only a GET for the stored ID.
    assert!(ledger::reserve(&state, &run, MediaKind::Image, &route).is_err());
    let stored = ledger::get(&state, &run).unwrap().unwrap();
    assert_eq!(stored["jobId"], "job_fixture");
    let resumed = replicate::generate(
        &state,
        &run,
        MediaKind::Image,
        "",
        &route,
        receiver,
        &channel,
        stored["jobId"].as_str(),
    )
    .await;
    ledger::finish(&state, &run, &route, &resumed).unwrap();
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    assert_eq!(gets.load(Ordering::SeqCst), 1);
    assert_eq!(
        ledger::cached(&state, &run, 0).unwrap().unwrap(),
        [1, 2, 3, 4]
    );
    assert_eq!(
        ledger::get(&state, &run).unwrap().unwrap()["status"],
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
        axum::routing::post(|| async { axum::Json(json!({"status":"processing"})) }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (state, route) = state(&format!("http://{address}/v1"));
    let run = uuid::Uuid::new_v4().to_string();
    ledger::reserve(&state, &run, MediaKind::Image, &route).unwrap();
    ledger::phase(&state, &run, "running", Some("job_fixture")).unwrap();
    let error = replicate::cancel_existing(&state, &route, "job_fixture").await;
    assert!(error.may_have_generated);
    assert_eq!(error.code, "remote_cancel_unconfirmed");
    ledger::finish(&state, &run, &route, &Err(error)).unwrap();
    assert_eq!(
        ledger::get(&state, &run).unwrap().unwrap()["status"],
        "unknown"
    );
    ledger::cache(&state, &run, 0, &[1, 2]).unwrap();
    state
        .sqlite_writer
        .write(|db| {
            let loaded = persistence::service_registry_store::load_registry(db)?;
            let mut next = loaded.snapshot;
            next.bindings
                .iter_mut()
                .find(|b| b.purpose == Purpose::MediaImageGenerate)
                .unwrap()
                .cloud_allowed = false;
            persistence::service_registry_store::save_registry(db, &next, loaded.revision)?;
            Ok(())
        })
        .unwrap();
    let result = Ok(MediaResult {
        kind: MediaKind::Image,
        model: route.model.clone(),
        job_id: Some("job_fixture".into()),
        artifacts: vec![],
    });
    assert!(ledger::finish(&state, &run, &route, &result).is_err());
    assert!(ledger::cached(&state, &run, 0).unwrap().is_none());
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
    let (_, route) = state(&format!("http://{address}/v1"));
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
