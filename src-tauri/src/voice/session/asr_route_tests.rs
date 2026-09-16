use super::*;
use axum::{routing::post, Router};
use std::sync::atomic::{AtomicUsize, Ordering};
async fn fixture(
    status: u16,
    body: &'static str,
    delay_ms: u64,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let count = hits.clone();
    let app = Router::new()
        .route(
            "/primary/audio/transcriptions",
            post(move || async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    [("content-type", "application/json")],
                    body,
                )
            }),
        )
        .route(
            "/fallback/audio/transcriptions",
            post(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    axum::Json(serde_json::json!({"text":"hello","languages":[{"code":"ja"}]}))
                }
            }),
        );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, hits, server)
}
fn selected(base: &str) -> SelectedAsr {
    let provider = |prefix: &str| {
        AsrRoute::Cloud(crate::CloudAsrProviderSettings {
            id: prefix.into(),
            enabled: true,
            label: prefix.into(),
            location: "local".into(),
            endpoint: format!("{base}/{prefix}"),
            model: "fixture".into(),
            language: "auto".into(),
            authentication: "none".into(),
        })
    };
    SelectedAsr {
        route: provider("primary"),
        fallbacks: vec![provider("fallback")],
        timeout_ms: 400,
        attempt_timeout_ms: 80,
        allowed_languages: vec!["ja".into()],
        vad_sensitivity: "medium".into(),
    }
}
#[tokio::test]
async fn transient_failure_and_slow_setup_fall_back_within_the_total_budget() {
    for (status, delay) in [(503, 0), (200, 500)] {
        let (base, hits, server) = fixture(status, "{}", delay).await;
        let started = std::time::Instant::now();
        let result = transcribe_routes(
            &NetworkAsrRuntime::new().unwrap(),
            &[0.1; 16000],
            16000,
            &selected(&base),
            Arc::default(),
        )
        .await
        .unwrap();
        assert_eq!(result.0, "hello");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_millis(400));
        server.abort();
    }
}
#[tokio::test]
async fn authentication_contract_language_and_cancellation_never_fall_back() {
    for (status, body) in [
        (401, "{}"),
        (422, "{}"),
        (
            200,
            "{\"text\":\"hello\",\"languages\":[{\"code\":\"en\"}]}",
        ),
        (200, "{\"text\":\"\"}"),
    ] {
        let (base, hits, server) = fixture(status, body, 0).await;
        assert!(transcribe_routes(
            &NetworkAsrRuntime::new().unwrap(),
            &[0.1; 16000],
            16000,
            &selected(&base),
            Arc::default()
        )
        .await
        .is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        server.abort();
    }
    let cancel = Arc::new(RunCancellation::default());
    cancel.cancel();
    assert!(transcribe_routes(
        &NetworkAsrRuntime::new().unwrap(),
        &[0.1; 16000],
        16000,
        &selected("http://127.0.0.1:1"),
        cancel
    )
    .await
    .unwrap_err()
    .contains("cancelled"));
}
