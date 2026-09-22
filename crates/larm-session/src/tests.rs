use super::*;
use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
    Json, Router,
};
use serde_json::Value;
use std::sync::{atomic::AtomicUsize, Mutex};
const GEMMA4_KV_TOKENS: u64 = 225 * 1024;
struct Fake {
    base: String,
    generation: AtomicUsize,
    log: Mutex<Vec<String>>,
    bad_claim: AtomicBool,
    pending: AtomicBool,
    stale_health: AtomicBool,
    bad_protocol: AtomicBool,
    slow_claim: AtomicBool,
    slow_create: AtomicBool,
    fail_release: AtomicBool,
    wrong_renew_id: AtomicBool,
    released: AtomicBool,
}
impl Fake {
    fn state(&self, status: &str) -> Value {
        json!({"id":"session-1","status":status,"expiresAt":(chrono::Utc::now()+chrono::Duration::seconds(600)).to_rfc3339()})
    }
    fn claim(&self) -> Value {
        let generation = self.generation.load(Ordering::SeqCst);
        let mut providers = contract::PROVIDERS.iter().rev().map(|(name, protocol)| json!({
            "name":name,"protocol":protocol,
            "baseUrl":format!("{}/{name}/v1",self.base),
            "model":format!("claimed-{name}-{generation}"),
            "configuration":{"fields":{"baseURL":"http://localhost/ignored","model":"ignored"}},
            "credential":{"token":format!("token-{name}-{generation}")},
            "health":{"url":format!("{}/{name}/health",self.base),"maxAgeMs":10000},
            "contextWindow": if *name == "llm" {
                json!({"maxTokens":GEMMA4_KV_TOKENS,"outputReserveTokens":4096,"safetyMarginTokens":1976})
            } else { Value::Null },
            "embeddingSpace": if *name == "embedding" { json!({"dimension":384}) } else { Value::Null }
        })).collect::<Vec<_>>();
        if self.bad_claim.load(Ordering::SeqCst) {
            providers[0]["protocol"] = json!("invalid-protocol");
        }
        let mut value = self.state("ready");
        value["allocationId"] = json!(format!("allocation-{generation}"));
        value["providers"] = json!(providers);
        value
    }
}
async fn handle(State(fake): State<Arc<Fake>>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().to_string();
    fake.log.lock().unwrap().push(format!("{method} {path}"));
    if path.starts_with("/v1/agent-connections") {
        assert_eq!(
            request.headers()["authorization"],
            "Bearer test-control-token"
        );
        if method == "DELETE" {
            if fake.fail_release.load(Ordering::SeqCst) {
                return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
            fake.released.store(true, Ordering::SeqCst);
            return (axum::http::StatusCode::NO_CONTENT, "").into_response();
        }
        if path.ends_with("/claim") {
            let body = axum::body::to_bytes(request.into_body(), 10000)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap(),
                json!({"format":"openai-provider-v1"})
            );
            if fake.slow_claim.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            return Json(fake.claim()).into_response();
        }
        if method == "POST" {
            let prefix = if path.ends_with("/renew") {
                "saaa-renew-"
            } else {
                "saaa-session-"
            };
            assert!(request.headers()["idempotency-key"]
                .to_str()
                .unwrap()
                .starts_with(prefix));
            let body = axum::body::to_bytes(request.into_body(), 10000)
                .await
                .unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["ttlSeconds"], 600);
            if path.ends_with("/renew") {
                fake.generation.fetch_add(1, Ordering::SeqCst);
                let mut value = fake.state("ready");
                if fake.wrong_renew_id.load(Ordering::SeqCst) {
                    value["id"] = json!("another-session");
                }
                return Json(value).into_response();
            }
            assert_eq!(
                value,
                json!({"agentProfile":"saaa-conversation-gemma4","explicitAgentProfile":true,
                "audience":"saaa-desktop","client":"saaa-coding-agent","ttlSeconds":600,
                "allowFallback":false,"deploymentPolicy":"existing-only"})
            );
            if fake.slow_create.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            return (
                axum::http::StatusCode::ACCEPTED,
                Json(fake.state(if fake.pending.load(Ordering::SeqCst) {
                    "pending"
                } else {
                    "ready"
                })),
            )
                .into_response();
        }
        return Json(fake.state(if fake.pending.load(Ordering::SeqCst) {
            "probing"
        } else {
            "ready"
        }))
        .into_response();
    }
    let name = path.split('/').nth(1).unwrap();
    if fake.released.load(Ordering::SeqCst) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    let expected_token = format!(
        "Bearer token-{name}-{}",
        fake.generation.load(Ordering::SeqCst)
    );
    if request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected_token.as_str())
    {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    if path.ends_with("/embed") {
        let body = axum::body::to_bytes(request.into_body(), 100_000)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"texts":["埋め込みテスト"],"type":"query","normalize":true,"priority":"normal"})
        );
        return Json(json!({"embeddings":[vec![0.0_f32;384]]})).into_response();
    }
    let protocol = contract::PROVIDERS
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1;
    Json(json!({"ready":!fake.stale_health.load(Ordering::SeqCst),"acceptingRequests":true,
        "capacity":{"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":1,
            "queueDepth":0,"queueTimeoutMs":1000,"retryAfterMs":0,"completionGuaranteed":false},
        "probe":{"validated":true,"protocol":if fake.bad_protocol.load(Ordering::SeqCst) {"wrong"} else {protocol}}})).into_response()
}
async fn fixture() -> (Arc<Fake>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fake = Arc::new(Fake {
        base: format!("http://{}", listener.local_addr().unwrap()),
        generation: AtomicUsize::new(0),
        log: Mutex::new(vec![]),
        bad_claim: AtomicBool::new(false),
        pending: AtomicBool::new(false),
        stale_health: AtomicBool::new(false),
        bad_protocol: AtomicBool::new(false),
        slow_claim: AtomicBool::new(false),
        slow_create: AtomicBool::new(false),
        fail_release: AtomicBool::new(false),
        wrong_renew_id: AtomicBool::new(false),
        released: AtomicBool::new(false),
    });
    let app = Router::new().fallback(handle).with_state(fake.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (fake, server)
}
fn count(fake: &Fake, suffix: &str) -> usize {
    fake.log
        .lock()
        .unwrap()
        .iter()
        .filter(|l| l.ends_with(suffix))
        .count()
}
#[tokio::test]
async fn maps_reordered_providers_and_caches_health_then_releases_once() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    for (name, _) in contract::PROVIDERS {
        let lease = session.acquire(name).await.unwrap();
        assert_eq!(lease.provider().model, format!("claimed-{name}-0"));
        assert_eq!(lease.provider().token(), format!("token-{name}-0"));
        assert_eq!(lease.allocation_id(), "allocation-0");
        match name {
            "llm" => {
                let window = lease.provider().context_window.unwrap();
                assert_eq!(window.max_tokens, GEMMA4_KV_TOKENS);
                assert_eq!(window.output_reserve_tokens, 4096);
                assert_eq!(window.safety_margin_tokens, 1976);
                assert_eq!(window.max_input_tokens(), 224328);
            }
            "embedding" => {
                assert!(lease.provider().context_window.is_none());
                assert_eq!(lease.provider().embedding_space.unwrap().dimension, 384);
            }
            "asr" | "tts" => {
                assert!(lease.provider().context_window.is_none());
                assert!(lease.provider().embedding_space.is_none());
            }
            _ => unreachable!(),
        }
        assert_eq!(count(&fake, &format!("/{name}/health")), 1);
    }
    session.close().await.unwrap();
    session.close().await.unwrap();
    assert!(session.acquire("llm").await.is_err());
    assert_eq!(
        fake.log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("DELETE"))
            .count(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn embedding_uses_claimed_endpoint_model_space_and_bearer() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    let vectors = session
        .embed_query(&["埋め込みテスト".into()])
        .await
        .unwrap();
    assert_eq!(vectors.len(), 1);
    assert_eq!(vectors[0].len(), 384);
    assert_eq!(count(&fake, "/embedding/v1/embed"), 1);
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn released_provider_tokens_are_rejected() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    let lease = session.acquire("embedding").await.unwrap();
    let endpoint = lease.provider().endpoint("embed").unwrap();
    let token = lease.provider().token().to_string();
    drop(lease);
    session.close().await.unwrap();
    let status = reqwest::Client::new()
        .post(endpoint)
        .bearer_auth(token)
        .json(
            &json!({"texts":["after release"],"type":"query","normalize":true,"priority":"normal"}),
        )
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
    server.abort();
}

#[tokio::test]
async fn provider_capacity_one_serializes_requests_and_does_not_imply_guaranteed_completion() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    let first = session.acquire("llm").await.unwrap();
    assert_eq!(first.capacity().unwrap().max_concurrent_requests, 1);
    assert!(!first.capacity().unwrap().completion_guaranteed);
    let cloned = session.clone();
    let second = tokio::spawn(async move { cloned.acquire("llm").await });
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(!second.is_finished());
    drop(first);
    let second = tokio::time::timeout(Duration::from_secs(1), second)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    drop(second);
    session.close().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn renew_waits_for_inflight_use_and_atomically_changes_all_tokens() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    let mut old_credentials = Vec::new();
    for (name, _) in contract::PROVIDERS {
        let lease = session.acquire(name).await.unwrap();
        old_credentials.push((
            lease.provider().health_url.clone(),
            lease.provider().token().to_string(),
        ));
    }
    // Arrange expiry while the snapshot is exclusive, then pin a simulated in-flight request.
    session.snapshot.write().await.as_mut().unwrap().expires_at =
        chrono::Utc::now() + chrono::Duration::seconds(60);
    let guard = session.snapshot.clone().read_owned().await;
    let clone = session.clone();
    let renewal = tokio::spawn(async move { clone.renew_if_due().await });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(count(&fake, "/renew"), 0);
    drop(guard);
    renewal.await.unwrap().unwrap();
    for (name, _) in contract::PROVIDERS {
        let lease = session.acquire(name).await.unwrap();
        assert_eq!(lease.provider().token(), format!("token-{name}-1"));
        assert_eq!(lease.allocation_id(), "allocation-1");
    }
    assert_eq!(count(&fake, "/renew"), 1);
    for (health_url, token) in old_credentials {
        let status = reqwest::Client::new()
            .get(health_url)
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, axum::http::StatusCode::UNAUTHORIZED);
    }
    session.close().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn dropped_renew_waiter_does_not_abandon_reclaim() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    fake.slow_claim.store(true, Ordering::SeqCst);
    session.snapshot.write().await.as_mut().unwrap().expires_at =
        chrono::Utc::now() + chrono::Duration::seconds(60);
    let clone = session.clone();
    let renewal = tokio::spawn(async move { clone.renew_if_due().await });
    while count(&fake, "/renew") == 0 {
        tokio::task::yield_now().await;
    }
    renewal.abort();
    let lease = session.acquire("llm").await.unwrap();
    assert_eq!(lease.provider().token(), "token-llm-1");
    drop(lease);
    session.close().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn failed_reclaim_never_restores_old_tokens_and_releases() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    fake.bad_claim.store(true, Ordering::SeqCst);
    session.snapshot.write().await.as_mut().unwrap().expires_at =
        chrono::Utc::now() + chrono::Duration::seconds(60);
    assert!(session.renew_if_due().await.is_err());
    assert!(session.acquire("llm").await.is_err());
    assert!(session.snapshot.read().await.is_none());
    assert!(fake
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.starts_with("DELETE")));
    server.abort();
}
#[tokio::test]
async fn startup_failure_and_cancel_both_release_the_created_id() {
    for cancel in [false, true] {
        let (fake, server) = fixture().await;
        let (stop, receiver) = watch::channel(false);
        if cancel {
            fake.pending.store(true, Ordering::SeqCst);
        } else {
            fake.bad_claim.store(true, Ordering::SeqCst);
        }
        let base = fake.base.clone();
        let start = tokio::spawn(async move { Session::connect(&base, receiver).await });
        if cancel {
            while count(&fake, "/v1/agent-connections") == 0 {
                tokio::task::yield_now().await;
            }
            stop.send_replace(true);
        }
        assert!(start.await.unwrap().is_err());
        assert!(fake
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.starts_with("DELETE")));
        server.abort();
    }
}
#[tokio::test]
async fn health_failure_is_confined_to_the_requested_capability() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    {
        let snapshot = session.snapshot.read().await;
        let provider = &snapshot.as_ref().unwrap().providers["llm"];
        *provider.checked_at.lock().await = Some(Instant::now() - Duration::from_secs(11));
    }
    fake.stale_health.store(true, Ordering::SeqCst);
    let initial_health_checks = count(&fake, "/llm/health");
    assert!(session.acquire("llm").await.is_err());
    assert_eq!(count(&fake, "/llm/health"), initial_health_checks + 1);
    fake.stale_health.store(false, Ordering::SeqCst);
    assert!(session.acquire("asr").await.is_ok());
    assert!(!session.closed.load(Ordering::Acquire));
    session.close().await.unwrap();
    server.abort();
}
#[tokio::test]
async fn rejects_duplicate_missing_and_nonlocal_provider_contracts() {
    let (fake, server) = fixture().await;
    let mut value = fake.claim();
    value["providers"][1] = value["providers"][0].clone();
    assert!(contract::parse(value, "session-1").is_err());
    let mut value = fake.claim();
    value["providers"][0]["baseUrl"] = json!("https://example.com/v1");
    assert!(contract::parse(value, "session-1").is_err());
    assert!(local_url(
        &url::Url::parse("http://gnosis.local:9810").unwrap()
    ));
    server.abort();
}

#[tokio::test]
async fn abandoning_creation_still_receives_the_id_and_releases_it() {
    let (fake, server) = fixture().await;
    fake.slow_create.store(true, Ordering::SeqCst);
    fake.pending.store(true, Ordering::SeqCst);
    let (_stop, receiver) = watch::channel(false);
    let base = fake.base.clone();
    let caller = tokio::spawn(async move { Session::connect(&base, receiver).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while count(&fake, "/v1/agent-connections") == 0 {
            tokio::task::yield_now().await;
        }
        caller.abort();
        while !fake
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("DELETE"))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("abandoned creation must release without waiting for the startup timeout");
    server.abort();
}
#[tokio::test]
async fn startup_release_failure_keeps_a_retryable_handle() {
    let (fake, server) = fixture().await;
    fake.bad_claim.store(true, Ordering::SeqCst);
    fake.fail_release.store(true, Ordering::SeqCst);
    let (_stop, receiver) = watch::channel(false);
    let error = Session::connect(&fake.base, receiver)
        .await
        .err()
        .expect("startup failure");
    let session = error
        .cleanup
        .expect("release failure must retain the lease");
    assert!(session.acquire("llm").await.is_err());
    fake.fail_release.store(false, Ordering::SeqCst);
    session.close().await.unwrap();
    assert!(session.released.load(Ordering::Acquire));
    server.abort();
}
#[tokio::test]
async fn abandoning_close_does_not_abandon_release_behind_an_inflight_request() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    let use_guard = session.acquire("llm").await.unwrap();
    let cloned = session.clone();
    let closer = tokio::spawn(async move { cloned.close().await });
    while !session.closed.load(Ordering::Acquire) {
        tokio::task::yield_now().await;
    }
    closer.abort();
    drop(use_guard);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !session.released.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("close must finish even after its waiter is dropped");
    server.abort();
}
#[tokio::test]
async fn renew_rejects_a_response_for_a_different_connection() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    fake.wrong_renew_id.store(true, Ordering::SeqCst);
    session.snapshot.write().await.as_mut().unwrap().expires_at =
        chrono::Utc::now() + chrono::Duration::seconds(60);
    assert_eq!(
        session.renew_if_due().await,
        Err("larm_connection_mismatch")
    );
    assert!(session.released.load(Ordering::Acquire));
    server.abort();
}

#[tokio::test]
async fn claim_requires_the_complete_saaa_provider_set() {
    let (fake, server) = fixture().await;
    let mut value = fake.claim();
    value["providers"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["name"] == "llm");
    assert_eq!(
        contract::parse(value, "session-1").err(),
        Some("larm_missing_provider")
    );
    server.abort();
}

#[tokio::test]
async fn chat_context_window_is_mandatory_and_validated_but_audio_does_not_require_it() {
    let (fake, server) = fixture().await;
    let mut missing = fake.claim();
    missing["providers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|provider| provider["name"] == "llm")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("contextWindow");
    assert_eq!(
        contract::parse(missing, "session-1").err(),
        Some("larm_missing_context_window")
    );

    let mut invalid = fake.claim();
    let chat = invalid["providers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|provider| provider["name"] == "llm")
        .unwrap();
    chat["contextWindow"]["safetyMarginTokens"] = json!(230400);
    assert_eq!(
        contract::parse(invalid, "session-1").err(),
        Some("larm_invalid_context_window")
    );
    server.abort();
}

#[tokio::test]
async fn embedding_space_is_mandatory_and_validated() {
    let (fake, server) = fixture().await;
    let mut missing = fake.claim();
    let embedding = missing["providers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|provider| provider["name"] == "embedding")
        .unwrap();
    embedding.as_object_mut().unwrap().remove("embeddingSpace");
    assert_eq!(
        contract::parse(missing, "session-1").err(),
        Some("larm_missing_embedding_space")
    );

    let mut invalid = fake.claim();
    let embedding = invalid["providers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|provider| provider["name"] == "embedding")
        .unwrap();
    embedding["embeddingSpace"]["dimension"] = json!(0);
    assert_eq!(
        contract::parse(invalid, "session-1").err(),
        Some("larm_invalid_embedding_space")
    );
    server.abort();
}

#[tokio::test]
async fn audio_request_budget_cannot_outlive_the_pinned_credential() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    session.snapshot.write().await.as_mut().unwrap().expires_at =
        chrono::Utc::now() + chrono::Duration::seconds(100);
    let lease = session.acquire("tts").await.unwrap();
    let budget = lease.request_budget(Duration::from_secs(300)).unwrap();
    assert!(budget <= Duration::from_secs(95));
    assert!(budget > Duration::from_secs(90));
    assert_eq!(
        lease.request_budget(Duration::from_secs(1)).unwrap(),
        Duration::from_secs(1)
    );
    drop(lease);
    session.close().await.unwrap();
    server.abort();
}
