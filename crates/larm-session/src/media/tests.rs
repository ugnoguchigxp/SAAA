use super::*;
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
use std::sync::{Arc, Mutex};
mod regression;
struct Fake {
    log: Mutex<Vec<String>>,
    posts: Mutex<Vec<Value>>,
    mode: &'static str,
    polls: Mutex<usize>,
}
async fn handle(State(fake): State<Arc<Fake>>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().to_string();
    assert_eq!(request.headers()["authorization"], "Bearer secret-control");
    fake.log.lock().unwrap().push(format!("{method} {path}"));
    if path == "/v3/agent-profiles" {
        let music = request.uri().query().unwrap().contains("SAAA-w-music");
        let kind = if music { "music" } else { "image" };
        return Json(json!({"contractVersion":"agent-connection.v3","catalogRevision":"revision",
            "requestedProfile":if music {"SAAA-w-music"} else {"SAAA-w-Image"},"profiles":[{
                "id":"gemma4-conversation","providers":[{"name":"llm","capability":"llm.general","protocol":"openai.chat-completions.v1","endpoint":"/warm/chat","model":"gemma-from-catalog","contextWindow":{"maxTokens":262144,"outputReserveTokens":4096,"safetyMarginTokens":1976}}],
                "services":[{"name":kind,"capability":format!("media.{kind}.generate"),"protocol":format!("larm.{kind}-generation.v1"),"endpoint":format!("/public/{kind}/generate"),"model":format!("discovered-{kind}"),"startupPolicy":{"minWarmInstances":0,"idleTtlSeconds":if music {300} else {120}}}]
            }]})).into_response();
    }
    if method == "POST" {
        let bytes = to_bytes(request.into_body(), 20000).await.unwrap();
        fake.posts
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&bytes).unwrap());
        if fake.mode == "conflict" {
            return (
                axum::http::StatusCode::CONFLICT,
                Json(json!({"error":{"code":"heavy_service_conflict","message":"busy"}})),
            )
                .into_response();
        }
        if fake.mode == "startup" {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error":{"code":"service_start_failed","message":"failed"}})),
            )
                .into_response();
        }
        if fake.mode == "failure" {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":{"code":"generation_failed","message":"failed"}})),
            )
                .into_response();
        }
        if fake.mode == "delayed" || fake.mode == "slow_submission" {
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        if path == "/public/image/generate" {
            if fake.mode == "invalid_submission_json" {
                return Response::builder().body(Body::from("{broken")).unwrap();
            }
            if fake.mode == "foreign_artifact" {
                return Json(json!({"artifacts":[{"id":"image-1","contentUrl":"http://127.0.0.1:8810/private","mimeType":"image/png"}]})).into_response();
            }
            return Json(if fake.mode == "warm_only" { json!({"status":"ready","claimable":true}) }
                else { json!({"data":[{"artifact":{"id":"image-1","contentUrl":"/stored/image-1","mimeType":"image/png"}}]}) }).into_response();
        }
        let mut response = (
            axum::http::StatusCode::ACCEPTED,
            Json(json!({"jobId":"job-1","status":"loading"})),
        )
            .into_response();
        if fake.mode == "location" {
            response
                .headers_mut()
                .insert("location", "/jobs/music/job-1".parse().unwrap());
        }
        return response;
    }
    if path == "/public/music/generate/job-1" || path == "/jobs/music/job-1" {
        if method == "DELETE" {
            if fake.mode == "cancel_failure" {
                return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            return Json(json!({"jobId":"job-1","status":"cancelled"})).into_response();
        }
        if fake.mode == "poll_failure" {
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        let poll = {
            let mut count = fake.polls.lock().unwrap();
            *count += 1;
            *count
        };
        if fake.mode == "music_startup" {
            return Json(
                json!({"jobId":"job-1","status":"failed","error":{"code":"model_load_failed"}}),
            )
            .into_response();
        }
        if fake.mode == "music_failure" {
            return Json(
                json!({"jobId":"job-1","status":"failed","error":{"code":"generation_failed"}}),
            )
            .into_response();
        }
        if matches!(fake.mode, "job_timeout" | "cancel_failure") || poll == 1 {
            return Json(json!({"jobId":"job-1","status":"generating","progress":0.5}))
                .into_response();
        }
        return Json(json!({"jobId":"job-1","status":"completed","result":{"id":"audio-1","audioUrl":"/stored/audio-1","metadataUrl":"/stored/meta-1","format":"mp3"}})).into_response();
    }
    if path == "/stored/meta-1" {
        if fake.mode == "metadata_failure" {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
        if fake.mode == "metadata_mismatch" {
            return Json(json!({"id":"another-artifact"})).into_response();
        }
        return Json(json!({"id":"audio-1","prompt":"metadata"})).into_response();
    }
    if path.starts_with("/stored/") {
        if fake.mode == "artifact_failure" {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
        let mime = if fake.mode == "wrong_content_type" {
            "text/html"
        } else if path.contains("image") {
            "image/png"
        } else {
            "audio/mpeg"
        };
        return Response::builder()
            .header("content-type", mime)
            .body(Body::from(vec![1_u8, 2, 3]))
            .unwrap();
    }
    panic!("Unexpected public API call: {method} {path}");
}
async fn fixture(
    mode: &'static str,
    kind: MediaKind,
) -> (Arc<Fake>, MediaClient, tokio::task::JoinHandle<()>) {
    let fake = Arc::new(Fake {
        log: Mutex::new(vec![]),
        posts: Mutex::new(vec![]),
        mode,
        polls: Mutex::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().fallback(any(handle)).with_state(fake.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut client = MediaClient::discover(&base, "secret-control".into(), kind)
        .await
        .unwrap();
    client.limits.poll = Duration::from_millis(5);
    (fake, client, server)
}
#[tokio::test]
async fn discovery_preserves_cold_policy_and_does_not_start_or_health_check_service() {
    for kind in [MediaKind::Image, MediaKind::Music] {
        let (fake, client, server) = fixture("success", kind).await;
        assert!(client.service().starts_on_request());
        assert_eq!(client.profile.providers[0].model, "gemma-from-catalog");
        assert_eq!(
            fake.log.lock().unwrap().as_slice(),
            ["GET /v3/agent-profiles"]
        );
        assert_eq!(
            client.service().startup_policy.as_ref().unwrap().details["idleTtlSeconds"],
            if kind == MediaKind::Image { 120 } else { 300 }
        );
        server.abort();
    }
}
#[tokio::test]
async fn image_waits_for_cold_start_uses_discovered_model_and_fetches_artifact() {
    let (fake, client, server) = fixture("delayed", MediaKind::Image).await;
    let (_cancel, receiver) = watch::channel(false);
    let phases = Mutex::new(Vec::new());
    let on_progress = |event: MediaProgress| phases.lock().unwrap().push(event.phase);
    let operation = client.generate("draw", receiver, &on_progress);
    tokio::pin!(operation);
    assert!(
        tokio::time::timeout(Duration::from_millis(15), &mut operation)
            .await
            .is_err()
    );
    let result = operation.await.unwrap();
    assert_eq!(
        client.artifact_bytes(&result.artifacts[0]).await.unwrap(),
        [1, 2, 3]
    );
    assert_eq!(phases.lock().unwrap().as_slice(), ["starting"]);
    assert_eq!(fake.posts.lock().unwrap()[0]["model"], "discovered-image");
    assert_eq!(
        fake.log.lock().unwrap().as_slice(),
        [
            "GET /v3/agent-profiles",
            "POST /public/image/generate",
            "GET /stored/image-1"
        ]
    );
    server.abort();
}
#[tokio::test]
async fn music_follows_loading_and_generation_until_metadata_and_audio_are_retrieved() {
    let (fake, client, server) = fixture("success", MediaKind::Music).await;
    let (_cancel, receiver) = watch::channel(false);
    let phases = Mutex::new(Vec::new());
    let result = client
        .generate("music", receiver, &|event| {
            phases.lock().unwrap().push(event.phase)
        })
        .await
        .unwrap();
    assert_eq!(result.job_id.as_deref(), Some("job-1"));
    assert_eq!(
        client
            .artifact_metadata(&result.artifacts[0])
            .await
            .unwrap()["prompt"],
        "metadata"
    );
    assert_eq!(
        client.artifact_bytes(&result.artifacts[0]).await.unwrap(),
        [1, 2, 3]
    );
    assert_eq!(
        phases.lock().unwrap().as_slice(),
        ["starting", "loading", "generating", "completed"]
    );
    assert_eq!(fake.posts.lock().unwrap().len(), 1);
    assert_eq!(fake.posts.lock().unwrap()[0]["model"], "discovered-music");
    server.abort();
}
#[tokio::test]
async fn startup_conflict_and_generation_failures_are_distinct_and_never_resubmitted() {
    for kind in [MediaKind::Image, MediaKind::Music] {
        for (mode, expected) in [
            ("startup", FailureKind::StartupFailed),
            ("conflict", FailureKind::Conflict),
            ("failure", FailureKind::GenerationFailed),
        ] {
            let (fake, client, server) = fixture(mode, kind).await;
            let (_cancel, receiver) = watch::channel(false);
            let error = client
                .generate("prompt", receiver, &|_| {})
                .await
                .unwrap_err();
            assert_eq!(error.kind, expected);
            assert!(error.retryable);
            assert!(!error.may_have_generated);
            assert_eq!(fake.posts.lock().unwrap().len(), 1);
            server.abort();
        }
    }
}
#[tokio::test]
async fn music_job_failure_keeps_job_id_and_distinguishes_startup_from_generation() {
    for (mode, expected) in [
        ("music_startup", FailureKind::StartupFailed),
        ("music_failure", FailureKind::GenerationFailed),
    ] {
        let (fake, client, server) = fixture(mode, MediaKind::Music).await;
        let (_cancel, receiver) = watch::channel(false);
        let error = client
            .generate("prompt", receiver, &|_| {})
            .await
            .unwrap_err();
        assert_eq!(error.kind, expected);
        assert_eq!(error.job_id.as_deref(), Some("job-1"));
        assert_eq!(fake.posts.lock().unwrap().len(), 1);
        server.abort();
    }
}
#[tokio::test]
async fn image_cancel_and_timeout_report_unknown_remote_outcome() {
    for cancel_requested in [true, false] {
        let (fake, mut client, server) = fixture("delayed", MediaKind::Image).await;
        if !cancel_requested {
            client.limits.submission = Duration::from_millis(15);
        }
        let (cancel, receiver) = watch::channel(false);
        let send_cancel = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if cancel_requested {
                cancel.send_replace(true);
            }
        };
        let (result, _) = tokio::join!(client.generate("prompt", receiver, &|_| {}), send_cancel);
        let error = result.unwrap_err();
        assert_eq!(
            error.kind,
            if cancel_requested {
                FailureKind::Cancelled
            } else {
                FailureKind::Timeout
            }
        );
        assert!(error.may_have_generated);
        assert!(!error.retryable);
        assert_eq!(fake.posts.lock().unwrap().len(), 1);
        server.abort();
    }
}
#[tokio::test]
async fn music_cancel_during_submission_receives_job_then_requests_cancellation_once() {
    let (fake, client, server) = fixture("slow_submission", MediaKind::Music).await;
    let (cancel, receiver) = watch::channel(false);
    let cancellation = async {
        tokio::time::sleep(Duration::from_millis(15)).await;
        cancel.send_replace(true);
    };
    let (result, _) = tokio::join!(client.generate("prompt", receiver, &|_| {}), cancellation);
    let error = result.unwrap_err();
    assert_eq!(error.kind, FailureKind::Cancelled);
    assert!(!error.may_have_generated);
    assert_eq!(fake.posts.lock().unwrap().len(), 1);
    assert_eq!(
        fake.log.lock().unwrap().last().unwrap(),
        "DELETE /public/music/generate/job-1"
    );
    server.abort();
}
#[tokio::test]
async fn music_deadline_requests_cancel_and_reports_if_it_cannot_confirm() {
    for mode in ["job_timeout", "cancel_failure"] {
        let (fake, mut client, server) = fixture(mode, MediaKind::Music).await;
        client.limits.job = Duration::from_millis(18);
        let (_cancel, receiver) = watch::channel(false);
        let error = client
            .generate("prompt", receiver, &|_| {})
            .await
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::Timeout);
        assert_eq!(error.may_have_generated, mode == "cancel_failure");
        assert_eq!(error.job_id.as_deref(), Some("job-1"));
        assert_eq!(fake.posts.lock().unwrap().len(), 1);
        server.abort();
    }
}
#[tokio::test]
async fn artifact_failure_can_retry_get_without_generating_again() {
    let (fake, client, server) = fixture("artifact_failure", MediaKind::Image).await;
    let (_cancel, receiver) = watch::channel(false);
    let result = client.generate("prompt", receiver, &|_| {}).await.unwrap();
    for _ in 0..2 {
        assert_eq!(
            client
                .artifact_bytes(&result.artifacts[0])
                .await
                .unwrap_err()
                .kind,
            FailureKind::ArtifactFailed
        );
    }
    assert_eq!(fake.posts.lock().unwrap().len(), 1);
    server.abort();
}
#[tokio::test]
async fn warm_connection_success_is_not_an_image_generation_result() {
    let (_fake, client, server) = fixture("warm_only", MediaKind::Image).await;
    let (_cancel, receiver) = watch::channel(false);
    assert_eq!(
        client
            .generate("prompt", receiver, &|_| {})
            .await
            .unwrap_err()
            .kind,
        FailureKind::Protocol
    );
    server.abort();
}
#[test]
fn service_urls_cannot_leak_credentials_to_provider_ports_or_other_origins() {
    let base = Url::parse("http://127.0.0.1:9810/").unwrap();
    for endpoint in [
        "http://127.0.0.1:8810/generate",
        "https://evil.example/generate",
        "//evil.example/generate",
        "/generate?token=x",
    ] {
        assert!(catalog::public_url(&base, endpoint).is_err());
    }
}

#[tokio::test]
async fn completed_music_remains_successful_when_metadata_retrieval_fails() {
    let (fake, client, server) = fixture("metadata_failure", MediaKind::Music).await;
    let (_cancel, receiver) = watch::channel(false);
    let result = client.generate("prompt", receiver, &|_| {}).await.unwrap();
    for _ in 0..2 {
        assert_eq!(
            client
                .artifact_bytes(&result.artifacts[0])
                .await
                .unwrap_err()
                .kind,
            FailureKind::ArtifactFailed
        );
    }
    assert_eq!(fake.posts.lock().unwrap().len(), 1);
    server.abort();
}

#[tokio::test]
async fn music_uses_returned_job_location_instead_of_a_fixed_status_url() {
    let (fake, client, server) = fixture("location", MediaKind::Music).await;
    let (_cancel, receiver) = watch::channel(false);
    client.generate("prompt", receiver, &|_| {}).await.unwrap();
    let log = fake.log.lock().unwrap();
    assert!(log.iter().any(|line| line == "GET /jobs/music/job-1"));
    assert!(log
        .iter()
        .all(|line| !line.contains("/public/music/generate/job-1")));
    server.abort();
}

#[tokio::test]
async fn cancelling_before_submission_never_starts_the_cold_service() {
    let (fake, client, server) = fixture("success", MediaKind::Image).await;
    let (_cancel, receiver) = watch::channel(true);
    let error = client
        .generate("prompt", receiver, &|_| {})
        .await
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::Cancelled);
    assert!(fake.posts.lock().unwrap().is_empty());
    server.abort();
}
