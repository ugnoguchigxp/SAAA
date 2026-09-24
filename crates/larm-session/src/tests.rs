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
    profile: Mutex<String>,
    catalog: Mutex<Option<Value>>,
    catalog_gets: AtomicUsize,
    leases: AtomicUsize,
    claim_names: Mutex<Option<Vec<String>>>,
    omit_backchannel_window: AtomicBool,
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
        let profile = self.profile.lock().unwrap().clone();
        let names = self.claim_names.lock().unwrap().clone().unwrap_or_else(|| {
            let mut names: Vec<String> = contract::BASE_PROVIDERS
                .iter()
                .map(|(name, _)| (*name).to_string())
                .collect();
            if profile == contract::CANONICAL_PROFILE {
                names.push(contract::BACKCHANNEL.0.to_string());
            }
            names
        });
        let mut providers = names.into_iter().rev().map(|name| {
            let protocol = contract::accepted_provider(&name).unwrap_or("unknown");
            json!({
            "name":name,"protocol":protocol,
            "baseUrl":format!("{}/{name}/v1",self.base),
            "model": if name == "llm" && profile == contract::LEGACY_PROFILE { "gemma-4-fixture".to_string() } else { format!("claimed-{name}-{generation}") },
            "configuration":{"fields":{"baseURL":"http://localhost/ignored","model":"ignored"}},
            "credential":{"token":format!("token-{name}-{generation}")},
            "health":{"url":format!("{}/{name}/health",self.base),"maxAgeMs":10000},
            "contextWindow": if name == "llm" || (name == "backchannel" && !self.omit_backchannel_window.load(Ordering::SeqCst)) {
                json!({"maxTokens":GEMMA4_KV_TOKENS,"outputReserveTokens":4096,"safetyMarginTokens":1976})
            } else { Value::Null },
            "embeddingSpace": if name == "embedding" { json!({"dimension":384}) } else { Value::Null }
        })}).collect::<Vec<_>>();
        if self.omit_backchannel_window.load(Ordering::SeqCst) {
            for provider in &mut providers {
                if provider["name"] == "backchannel" {
                    provider.as_object_mut().unwrap().remove("contextWindow");
                }
            }
        }
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
    if path == "/v3/agent-profiles" {
        fake.catalog_gets.fetch_add(1, Ordering::SeqCst);
        return match fake.catalog.lock().unwrap().clone() {
            Some(value) => Json(value).into_response(),
            None => axum::http::StatusCode::NOT_FOUND.into_response(),
        };
    }
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
            fake.leases.fetch_sub(1, Ordering::SeqCst);
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
            assert_eq!(value["explicitAgentProfile"], true);
            assert_eq!(value["audience"], "saaa-desktop");
            assert_eq!(value["client"], "saaa-coding-agent");
            assert_eq!(value["allowFallback"], false);
            assert_eq!(value["deploymentPolicy"], "existing-only");
            let profile = value["agentProfile"].as_str().unwrap().to_string();
            *fake.profile.lock().unwrap() = profile;
            fake.leases.fetch_add(1, Ordering::SeqCst);
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
    let protocol = contract::accepted_provider(name).unwrap_or("unknown");
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
        profile: Mutex::new(String::new()),
        catalog: Mutex::new(None),
        catalog_gets: AtomicUsize::new(0),
        leases: AtomicUsize::new(0),
        claim_names: Mutex::new(None),
        omit_backchannel_window: AtomicBool::new(false),
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
    for name in contract::BASE_PROVIDERS.iter().map(|(name, _)| *name) {
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
    for name in contract::BASE_PROVIDERS.iter().map(|(name, _)| *name) {
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
    for name in contract::BASE_PROVIDERS.iter().map(|(name, _)| *name) {
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
    assert!(contract::parse(value, "session-1", &contract::required_providers("")).is_err());
    let mut value = fake.claim();
    value["providers"][0]["baseUrl"] = json!("https://example.com/v1");
    assert!(contract::parse(value, "session-1", &contract::required_providers("")).is_err());
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
        contract::parse(value, "session-1", &contract::required_providers(contract::PREVIOUS_DEFAULT_PROFILE)).err(),
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
        contract::parse(missing, "session-1", &contract::required_providers(contract::PREVIOUS_DEFAULT_PROFILE)).err(),
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
        contract::parse(invalid, "session-1", &contract::required_providers(contract::PREVIOUS_DEFAULT_PROFILE)).err(),
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
        contract::parse(missing, "session-1", &contract::required_providers(contract::PREVIOUS_DEFAULT_PROFILE)).err(),
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
        contract::parse(invalid, "session-1", &contract::required_providers(contract::PREVIOUS_DEFAULT_PROFILE)).err(),
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

fn catalog(entries: &[(&str, &[&str])]) -> Value {
    json!({
        "contractVersion": "agent-connection.v3",
        "profiles": entries.iter().map(|(id, names)| json!({
            "id": id,
            "providers": names.iter().map(|name| json!({"name": name})).collect::<Vec<_>>()
        })).collect::<Vec<_>>()
    })
}
const FIVE: &[&str] = &["llm", "backchannel", "asr", "tts", "embedding"];
const FOUR: &[&str] = &["llm", "asr", "tts", "embedding"];

async fn connect_auto(fake: &Fake) -> Result<Arc<Session>, ConnectError> {
    let (_stop, receiver) = watch::channel(false);
    Session::connect_with_profile_credential_and_key(
        &fake.base,
        ProfilePreference::Auto,
        "test-control-token".into(),
        format!("saaa-session-{}", uuid::Uuid::new_v4()),
        receiver,
    )
    .await
}

#[tokio::test]
async fn auto_selects_canonical_ornith15_profile() {
    let (fake, server) = fixture().await;
    *fake.catalog.lock().unwrap() = Some(catalog(&[
        (contract::CANONICAL_PROFILE, FIVE),
        (contract::LEGACY_PROFILE, FOUR),
        (contract::PREVIOUS_DEFAULT_PROFILE, FOUR),
    ]));
    let session = connect_auto(&fake).await.unwrap();
    assert_eq!(session.profile_id(), contract::CANONICAL_PROFILE);
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn auto_falls_back_to_legacy_only_when_canonical_absent() {
    let (fake, server) = fixture().await;
    *fake.catalog.lock().unwrap() = Some(catalog(&[
        (contract::LEGACY_PROFILE, FOUR),
        (contract::PREVIOUS_DEFAULT_PROFILE, FOUR),
    ]));
    let session = connect_auto(&fake).await.unwrap();
    assert_eq!(session.profile_id(), contract::LEGACY_PROFILE);
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn auto_skips_profile_missing_required_providers() {
    let (fake, server) = fixture().await;
    *fake.catalog.lock().unwrap() = Some(catalog(&[
        (contract::CANONICAL_PROFILE, &["llm", "backchannel", "asr", "tts"]),
        (contract::LEGACY_PROFILE, FOUR),
    ]));
    let session = connect_auto(&fake).await.unwrap();
    assert_eq!(session.profile_id(), contract::LEGACY_PROFILE);
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn auto_uses_previous_default_when_only_it_exists() {
    let (fake, server) = fixture().await;
    *fake.catalog.lock().unwrap() =
        Some(catalog(&[(contract::PREVIOUS_DEFAULT_PROFILE, FOUR)]));
    let session = connect_auto(&fake).await.unwrap();
    assert_eq!(session.profile_id(), contract::PREVIOUS_DEFAULT_PROFILE);
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn auto_fails_without_any_candidate() {
    let (fake, server) = fixture().await;
    *fake.catalog.lock().unwrap() = Some(catalog(&[("other", FOUR)]));
    let error = connect_auto(&fake).await.err().unwrap();
    assert_eq!(error.to_string(), "larm_profile_unavailable");
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
    assert!(!fake
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|line| line.starts_with("POST /v1/agent-connections")));
    server.abort();
}

#[tokio::test]
async fn explicit_profile_skips_catalog() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect_with_profile_credential_and_key(
        &fake.base,
        ProfilePreference::Explicit("custom-x".into()),
        "test-control-token".into(),
        format!("saaa-session-{}", uuid::Uuid::new_v4()),
        receiver,
    )
    .await
    .unwrap();
    assert_eq!(fake.catalog_gets.load(Ordering::SeqCst), 0);
    assert_eq!(session.profile_id(), "custom-x");
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn legacy_profile_model_comes_from_claim() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session =
        Session::connect_with_profile(&fake.base, contract::LEGACY_PROFILE, receiver)
            .await
            .unwrap();
    let lease = session.acquire("llm").await.unwrap();
    assert_eq!(lease.provider().model, "gemma-4-fixture");
    drop(lease);
    session.close().await.unwrap();
    server.abort();
}

#[test]
fn no_qwen38_literal_in_sources() {
    let sources = [
        include_str!("contract.rs"),
        include_str!("lib.rs"),
        include_str!("catalog.rs"),
    ];
    for source in sources {
        assert!(!source.contains("qwen3.8"));
        assert!(!source.contains("qwen-3.8"));
    }
}

#[tokio::test]
async fn claims_five_providers_in_any_order() {
    let (fake, server) = fixture().await;
    *fake.claim_names.lock().unwrap() = Some(
        ["embedding", "tts", "llm", "asr", "backchannel"]
            .into_iter()
            .map(str::to_string)
            .collect(),
    );
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    for name in ["embedding", "tts", "llm", "asr", "backchannel"] {
        let lease = session.acquire(name).await.unwrap();
        assert_eq!(lease.provider().token(), format!("token-{name}-0"));
        assert!(lease
            .provider()
            .base_url
            .as_str()
            .contains(&format!("/{name}/")));
    }
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn canonical_requires_backchannel() {
    let (fake, server) = fixture().await;
    *fake.claim_names.lock().unwrap() = Some(FOUR.iter().map(|name| (*name).to_string()).collect());
    let (_stop, receiver) = watch::channel(false);
    let error = Session::connect(&fake.base, receiver).await.err().unwrap();
    assert_eq!(error.to_string(), "larm_missing_provider");
    assert_eq!(
        fake.log
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.starts_with("DELETE"))
            .count(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn legacy_backchannel_is_optional() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect_with_profile(&fake.base, contract::LEGACY_PROFILE, receiver)
        .await
        .unwrap();
    assert!(!session.has_provider("backchannel").await);
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn backchannel_requires_context_window() {
    let (fake, server) = fixture().await;
    fake.omit_backchannel_window.store(true, Ordering::SeqCst);
    *fake.profile.lock().unwrap() = contract::CANONICAL_PROFILE.into();
    let value = fake.claim();
    assert_eq!(
        contract::parse(
            value,
            "session-1",
            &contract::required_providers(contract::CANONICAL_PROFILE)
        )
        .err(),
        Some("larm_missing_context_window")
    );
    server.abort();
}

#[tokio::test]
async fn renew_reclaims_all_five_and_release_leaves_no_lease() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    session.snapshot.write().await.as_mut().unwrap().expires_at =
        chrono::Utc::now() + chrono::Duration::seconds(60);
    session.renew_if_due().await.unwrap();
    assert_eq!(count(&fake, "/renew"), 1);
    assert_eq!(count(&fake, "/claim"), 2);
    session.close().await.unwrap();
    session.close().await.unwrap();
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
    server.abort();
}
