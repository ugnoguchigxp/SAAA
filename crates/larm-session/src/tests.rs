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
    llm_model: Mutex<Option<String>>,
    backchannel_max_tokens: Mutex<Option<u64>>,
    bad_claim: AtomicBool,
    pending: AtomicBool,
    stale_health: AtomicBool,
    bad_protocol: AtomicBool,
    slow_claim: AtomicBool,
    slow_create: AtomicBool,
    fail_release: AtomicBool,
    wrong_renew_id: AtomicBool,
    released: AtomicBool,
    idle_released: AtomicBool,
    partial_ready: AtomicBool,
    terminal_pending: AtomicBool,
    location_only: AtomicBool,
}
impl Fake {
    fn state(&self, status: &str) -> Value {
        let selector = self.profile.lock().unwrap().clone();
        let agent_profile = self
            .catalog
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|catalog| catalog["profiles"][0]["id"].as_str())
            .filter(|_| selector.starts_with("SAAA"))
            .unwrap_or(&selector)
            .to_string();
        let mut providers = contract::required_providers()
            .into_iter()
            .map(|name| {
                json!({
                    "name":name,"protocol":contract::accepted_provider(name).unwrap(),
                    "endpoint":contract::expected_endpoint(name).unwrap(),
                    "model":format!("claimed-{name}"),"readiness":status,
                    "claimable":status == "ready"
                })
            })
            .collect::<Vec<_>>();
        if status == "ready" && self.partial_ready.load(Ordering::SeqCst) {
            providers[0]["readiness"] = json!("probing");
            providers[0]["claimable"] = json!(false);
        }
        let services = match selector.as_str() {
            "SAAA-w-Image" => vec![service_decl("image")],
            "SAAA-w-music" => vec![service_decl("music")],
            _ => Vec::new(),
        };
        json!({"id":"session-1","profile":selector,"agentProfile":agent_profile,
            "providers":providers,"services":services,"status":status,
            "expiresAt":(chrono::Utc::now()+chrono::Duration::seconds(900)).to_rfc3339()})
    }
    fn claim(&self) -> Value {
        let generation = self.generation.load(Ordering::SeqCst);
        let profile = self.profile.lock().unwrap().clone();
        let names = self.claim_names.lock().unwrap().clone().unwrap_or_else(|| {
            contract::required_providers()
                .into_iter()
                .map(str::to_string)
                .collect()
        });
        let mut providers = names.into_iter().rev().map(|name| {
            let protocol = contract::accepted_provider(&name).unwrap_or("unknown");
            let base_url = format!("{}/{name}/v1", self.base);
            let model = if name == "llm" {
                self.llm_model.lock().unwrap().clone().unwrap_or_else(|| if profile == "saaa-qwen38" { "gemma-4-fixture".to_string() } else { format!("claimed-{name}") })
            } else { format!("claimed-{name}") };
            json!({
            "name":name,"protocol":protocol,
            "baseUrl":base_url,
            "model":model,
            "configuration":{"fields":if name == "embedding" {
                json!({"daemonURL":base_url,"model":model,"dimension":384})
            } else {
                json!({"baseURL":base_url,"model":model})
            }},
            "credential":{"token":format!("token-{name}-{generation}")},
            "health":{"url":format!("{}/{name}/health",self.base),"maxAgeMs":10000},
            "contextWindow": if name == "llm" || (name == "backchannel" && !self.omit_backchannel_window.load(Ordering::SeqCst)) {
                let max = if name == "backchannel" {
                    self.backchannel_max_tokens.lock().unwrap().unwrap_or(GEMMA4_KV_TOKENS)
                } else {
                    GEMMA4_KV_TOKENS
                };
                json!({"maxTokens":max,"outputReserveTokens":4096,"safetyMarginTokens":1976})
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
            let llm = providers
                .iter_mut()
                .find(|provider| provider["name"] == "llm")
                .unwrap();
            llm["protocol"] = json!("invalid-protocol");
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
    fake.log
        .lock()
        .unwrap()
        .push(format!("{method} {}", request.uri()));
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
            if !path.ends_with("/renew") {
                assert_eq!(request.headers()["prefer"], "wait=1");
            }
            let body = axum::body::to_bytes(request.into_body(), 10000)
                .await
                .unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                value["ttlSeconds"],
                if path.ends_with("/renew") { 600 } else { 900 }
            );
            if path.ends_with("/renew") {
                fake.generation.fetch_add(1, Ordering::SeqCst);
                let mut value = fake.state("ready");
                if fake.wrong_renew_id.load(Ordering::SeqCst) {
                    value["id"] = json!("another-session");
                }
                return Json(value).into_response();
            }
            assert!(value.get("explicitAgentProfile").is_none());
            assert!(value.get("agentProfile").is_none());
            assert_eq!(value["audience"], "saaa-desktop");
            assert_eq!(value["client"], "saaa-desktop");
            assert_eq!(value["allowFallback"], false);
            assert_eq!(value["deploymentPolicy"], "existing-only");
            if fake.catalog_gets.load(Ordering::SeqCst) == 0 {
                assert!(value.get("expectedCatalogRevision").is_none());
            } else {
                assert_eq!(value["expectedCatalogRevision"], "rev-fixture");
            }
            let profile = value["profile"].as_str().unwrap().to_string();
            *fake.profile.lock().unwrap() = profile;
            fake.leases.fetch_add(1, Ordering::SeqCst);
            if fake.slow_create.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let mut state = fake.state(if fake.pending.load(Ordering::SeqCst) {
                "pending"
            } else {
                "ready"
            });
            if fake.location_only.load(Ordering::SeqCst) {
                state.as_object_mut().unwrap().remove("id");
            }
            return (
                if fake.pending.load(Ordering::SeqCst) {
                    axum::http::StatusCode::ACCEPTED
                } else {
                    axum::http::StatusCode::CREATED
                },
                [("location", "/v1/agent-connections/session-1")],
                Json(state),
            )
                .into_response();
        }
        return Json(if fake.terminal_pending.load(Ordering::SeqCst) {
            let mut state = fake.state("failed");
            state["error"] = json!({"code":"semantic_probe_failed"});
            state
        } else if fake.idle_released.load(Ordering::SeqCst) {
            let mut state = fake.state("released");
            state["reason"] = json!("foreground_idle_timeout");
            state
        } else {
            fake.state(if fake.pending.load(Ordering::SeqCst) {
                "probing"
            } else {
                "ready"
            })
        })
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
        idle_released: AtomicBool::new(false),
        partial_ready: AtomicBool::new(false),
        terminal_pending: AtomicBool::new(false),
        location_only: AtomicBool::new(false),
        profile: Mutex::new(String::new()),
        catalog: Mutex::new(Some(catalog(&[("saaa-conversation-ornith15", FIVE)]))),
        catalog_gets: AtomicUsize::new(0),
        leases: AtomicUsize::new(0),
        claim_names: Mutex::new(None),
        omit_backchannel_window: AtomicBool::new(false),
        llm_model: Mutex::new(None),
        backchannel_max_tokens: Mutex::new(None),
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
        assert_eq!(lease.provider().model, format!("claimed-{name}"));
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
async fn foreground_idle_release_invalidates_every_claimed_credential() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    assert!(session.check_status().await.is_ok());
    fake.idle_released.store(true, Ordering::SeqCst);
    assert_eq!(
        session.check_status().await,
        Err("larm_connection_idle_released")
    );
    for name in contract::required_providers() {
        assert!(
            session.acquire(name).await.is_err(),
            "{name} retained a credential"
        );
    }
    session.close().await.unwrap();
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn connection_phase_reports_ready_then_idle_release() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let (phase, _) = watch::channel(ConnectionPhase::ModelPreparing);
    let session = Session::connect_with_profile_credential_key_and_phase(
        &fake.base,
        ProfilePreference::Variant(ProfileVariant::Conversation),
        "test-control-token".into(),
        "saaa-session-phase-test".into(),
        receiver,
        Some(phase.clone()),
    )
    .await
    .unwrap();
    assert_eq!(*phase.borrow(), ConnectionPhase::Ready);
    session.report_phase(&json!({"status":"pending","reason":"capacity_wait"}));
    assert_eq!(*phase.borrow(), ConnectionPhase::CapacityWaiting);
    session.report_phase(&json!({"status":"probing"}));
    assert_eq!(*phase.borrow(), ConnectionPhase::SemanticProbing);
    fake.idle_released.store(true, Ordering::SeqCst);
    assert_eq!(
        session.check_status().await,
        Err("larm_connection_idle_released")
    );
    assert_eq!(*phase.borrow(), ConnectionPhase::IdleReleased);
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn ready_status_with_one_unready_provider_never_claims() {
    let (fake, server) = fixture().await;
    fake.partial_ready.store(true, Ordering::SeqCst);
    let (_stop, receiver) = watch::channel(false);
    let result = Session::connect(&fake.base, receiver).await;
    assert_eq!(result.err().unwrap().code, "larm_invalid_provider");
    assert_eq!(count(&fake, "/v1/agent-connections/session-1/claim"), 0);
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn later_partial_readiness_revokes_the_previous_generation() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect(&fake.base, receiver).await.unwrap();
    fake.partial_ready.store(true, Ordering::SeqCst);
    assert_eq!(session.check_status().await, Err("larm_invalid_provider"));
    assert!(session.acquire("llm").await.is_err());
    assert!(session.acquire("asr").await.is_err());
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn terminal_poll_returns_bounded_reason_and_releases_connection() {
    let (fake, server) = fixture().await;
    fake.pending.store(true, Ordering::SeqCst);
    fake.terminal_pending.store(true, Ordering::SeqCst);
    let (_stop, receiver) = watch::channel(false);
    let error = Session::connect(&fake.base, receiver).await.err().unwrap();
    assert_eq!(error.code, "larm_startup_terminal");
    assert_eq!(error.reason.as_deref(), Some("semantic_probe_failed"));
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
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
    let providers = value["providers"].as_array_mut().unwrap();
    let llm = providers
        .iter()
        .find(|provider| provider["name"] == "llm")
        .unwrap()
        .clone();
    providers.push(llm);
    assert!(contract::parse(value, "session-1", &contract::required_providers()).is_err());
    let mut value = fake.claim();
    let llm = value["providers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|provider| provider["name"] == "llm")
        .unwrap();
    llm["baseUrl"] = json!("https://example.com/v1");
    assert!(contract::parse(value, "session-1", &contract::required_providers()).is_err());
    assert!(local_url(
        &url::Url::parse("http://192.168.0.130:9810").unwrap()
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
        contract::parse(value, "session-1", &contract::required_providers()).err(),
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
        contract::parse(missing, "session-1", &contract::required_providers()).err(),
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
        contract::parse(invalid, "session-1", &contract::required_providers()).err(),
        Some("larm_invalid_context_window")
    );
    server.abort();
}

#[tokio::test]
async fn embedding_space_is_mandatory_and_validated() {
    let (fake, server) = fixture().await;
    assert!(contract::parse(fake.claim(), "session-1", &contract::required_providers()).is_ok());
    let mut wrong_url = fake.claim();
    let embedding = wrong_url["providers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|provider| provider["name"] == "embedding")
        .unwrap();
    embedding["configuration"]["fields"]["daemonURL"] = json!("http://wrong.example/v1");
    assert_eq!(
        contract::parse(wrong_url, "session-1", &contract::required_providers()).err(),
        Some("larm_invalid_provider_configuration")
    );
    let mut missing = fake.claim();
    let embedding = missing["providers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|provider| provider["name"] == "embedding")
        .unwrap();
    embedding.as_object_mut().unwrap().remove("embeddingSpace");
    assert_eq!(
        contract::parse(missing, "session-1", &contract::required_providers()).err(),
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
        contract::parse(invalid, "session-1", &contract::required_providers()).err(),
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

fn provider_decl(name: &str, model: &str, max_tokens: Option<u64>) -> Value {
    let mut value = json!({
        "name": name,
        "capability": match name {
            "llm" => "llm.general",
            "backchannel" => "llm.backchannel.classifier",
            "asr" => "speech.stt",
            "tts" => "speech.tts",
            _ => "embedding",
        },
        "protocol": contract::accepted_provider(name).unwrap_or("unknown"),
        "endpoint": contract::expected_endpoint(name).unwrap_or("/invalid"),
        "model": model,
    });
    if let Some(max_tokens) = max_tokens {
        value["contextWindow"] = json!({
            "maxTokens": max_tokens,
            "outputReserveTokens": 4096,
            "safetyMarginTokens": 1976
        });
    }
    value
}

fn catalog(entries: &[(&str, &[&str])]) -> Value {
    let profiles = entries
        .iter()
        .map(|(id, names)| {
            let providers = names
                .iter()
                .map(|name| {
                    let window = matches!(*name, "llm" | "backchannel").then_some(GEMMA4_KV_TOKENS);
                    provider_decl(name, &format!("claimed-{name}"), window)
                })
                .collect::<Vec<_>>();
            json!({ "id": id, "providers": providers, "services": [] })
        })
        .collect::<Vec<_>>();
    json!({
        "contractVersion": "agent-connection.v3",
        "catalogRevision": "rev-fixture",
        "requestedProfile": "SAAA",
        "profiles": profiles
    })
}
const FIVE: &[&str] = &["llm", "backchannel", "asr", "tts", "embedding"];
const FOUR: &[&str] = &["llm", "asr", "tts", "embedding"];

async fn connect_variant(
    fake: &Fake,
    variant: ProfileVariant,
) -> Result<Arc<Session>, ConnectError> {
    let (_stop, receiver) = watch::channel(false);
    Session::connect_with_profile_credential_and_key(
        &fake.base,
        ProfilePreference::Variant(variant),
        "test-control-token".into(),
        format!("saaa-session-{}", uuid::Uuid::new_v4()),
        receiver,
    )
    .await
}

fn selector_catalog(selector: &str, services: &[&str]) -> Value {
    let mut response = catalog(&[("saaa-conversation-ornith15", FIVE)]);
    response["requestedProfile"] = json!(selector);
    response["profiles"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .for_each(|profile| {
            profile["services"] = json!(services
                .iter()
                .map(|name| service_decl(name))
                .collect::<Vec<_>>());
        });
    response
}

fn service_decl(name: &str) -> Value {
    let (protocol, endpoint) = match name {
        "image" => ("larm.image-generation.v1", "/v1/images/generations"),
        "music" => ("larm.music-generation.v1", "/v1/music/generations"),
        _ => ("invalid", "/invalid"),
    };
    json!({"name":name,"capability":format!("media.{name}.generate"),
        "protocol":protocol,"endpoint":endpoint,"model":name})
}

#[tokio::test]
async fn variant_queries_selector_and_creates_returned_id() {
    for variant in ProfileVariant::ALL {
        let (fake, server) = fixture().await;
        let selector = variant.selector();
        *fake.catalog.lock().unwrap() = Some(selector_catalog(selector, variant_services(variant)));
        let session = connect_variant(&fake, variant).await.unwrap();
        assert_eq!(session.profile_id(), "saaa-conversation-ornith15");
        assert!(fake
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|line| { line == &format!("GET /v3/agent-profiles?profile={selector}") }));
        session.close().await.unwrap();
        server.abort();
    }
}

fn variant_services(variant: ProfileVariant) -> &'static [&'static str] {
    match variant {
        ProfileVariant::Conversation => &[],
        ProfileVariant::Image => &["image"],
        ProfileVariant::Music => &["music"],
    }
}

#[tokio::test]
async fn selector_mismatch_is_rejected() {
    let (fake, server) = fixture().await;
    let mut response = selector_catalog("SAAA", &[]);
    response["requestedProfile"] = json!("contextStill");
    *fake.catalog.lock().unwrap() = Some(response);
    let error = connect_variant(&fake, ProfileVariant::Conversation)
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "larm_catalog_invalid");
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn multiple_profiles_are_rejected() {
    let (fake, server) = fixture().await;
    *fake.catalog.lock().unwrap() = Some(catalog(&[("first", FIVE), ("second", FIVE)]));
    let error = connect_variant(&fake, ProfileVariant::Conversation)
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "larm_catalog_invalid");
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn missing_backchannel_in_catalog_is_rejected() {
    let (fake, server) = fixture().await;
    *fake.catalog.lock().unwrap() = Some(catalog(&[("saaa-conversation-ornith15", FOUR)]));
    let error = connect_variant(&fake, ProfileVariant::Conversation)
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "larm_profile_unavailable");
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn variant_service_mismatch_is_rejected() {
    for (variant, services) in [
        (ProfileVariant::Image, &[][..]),
        (ProfileVariant::Conversation, &["image"][..]),
    ] {
        let (fake, server) = fixture().await;
        *fake.catalog.lock().unwrap() = Some(selector_catalog(variant.selector(), services));
        let error = connect_variant(&fake, variant).await.err().unwrap();
        assert_eq!(error.to_string(), "larm_profile_unavailable");
        assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
        server.abort();
    }
}

#[tokio::test]
async fn concrete_profile_id_is_not_sent_as_selector() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let error = Session::connect_with_profile_credential_and_key(
        &fake.base,
        ProfilePreference::Explicit("custom-x".into()),
        "test-control-token".into(),
        format!("saaa-session-{}", uuid::Uuid::new_v4()),
        receiver,
    )
    .await
    .err()
    .unwrap();
    assert_eq!(fake.catalog_gets.load(Ordering::SeqCst), 0);
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
    assert_eq!(error.to_string(), "larm_unknown_selector");
    server.abort();
}

#[tokio::test]
async fn direct_saaa_selector_creates_ready_without_discovery() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect_with_profile_credential_and_key(
        &fake.base,
        ProfilePreference::Explicit("SAAA".into()),
        "test-control-token".into(),
        format!("saaa-session-{}", uuid::Uuid::new_v4()),
        receiver,
    )
    .await
    .unwrap();
    assert_eq!(fake.catalog_gets.load(Ordering::SeqCst), 0);
    assert_eq!(session.profile_id(), "saaa-conversation-ornith15");
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn accepted_create_polls_location_until_ready() {
    let (fake, server) = fixture().await;
    fake.pending.store(true, Ordering::SeqCst);
    fake.location_only.store(true, Ordering::SeqCst);
    let (_stop, receiver) = watch::channel(false);
    let base = fake.base.clone();
    let connect = tokio::spawn(async move { Session::connect(&base, receiver).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while count(&fake, "/v1/agent-connections/session-1") == 0 {
            tokio::task::yield_now().await;
        }
        fake.pending.store(false, Ordering::SeqCst);
    })
    .await
    .unwrap();
    let session = connect.await.unwrap().unwrap();
    assert!(count(&fake, "/v1/agent-connections/session-1") >= 1);
    session.close().await.unwrap();
    server.abort();
}

#[tokio::test]
async fn create_response_rejects_missing_duplicate_and_invalid_provider_fields() {
    let (fake, server) = fixture().await;
    *fake.profile.lock().unwrap() = "SAAA".into();
    let original = fake.state("ready");
    let required = contract::required_providers();
    assert!(contract::validate_created(&original, "SAAA", &required, None).is_ok());
    let mut missing = original.clone();
    missing["providers"].as_array_mut().unwrap().pop();
    assert!(contract::validate_created(&missing, "SAAA", &required, None).is_err());
    let mut duplicate = original.clone();
    duplicate["providers"][0] = duplicate["providers"][1].clone();
    assert!(contract::validate_created(&duplicate, "SAAA", &required, None).is_err());
    for (field, value) in [
        ("protocol", json!("invalid")),
        ("endpoint", json!("/invalid")),
        ("model", json!("")),
        ("claimable", json!(false)),
    ] {
        let mut invalid = original.clone();
        invalid["providers"][0][field] = value;
        assert!(
            contract::validate_created(&invalid, "SAAA", &required, None).is_err(),
            "{field}"
        );
    }
    server.abort();
}

#[tokio::test]
async fn legacy_profile_id_uses_saaa_selector() {
    let (fake, server) = fixture().await;
    let (_stop, receiver) = watch::channel(false);
    let session = Session::connect_with_profile(&fake.base, "saaa-qwen38", receiver)
        .await
        .unwrap();
    let lease = session.acquire("llm").await.unwrap();
    assert_eq!(lease.provider().model, "claimed-llm");
    assert_eq!(session.selector(), Some("SAAA"));
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
async fn explicit_profile_requires_backchannel() {
    let (fake, server) = fixture().await;
    *fake.claim_names.lock().unwrap() = Some(FOUR.iter().map(|name| (*name).to_string()).collect());
    let (_stop, receiver) = watch::channel(false);
    let error = Session::connect_with_profile_credential_and_key(
        &fake.base,
        ProfilePreference::Explicit("SAAA".into()),
        "test-control-token".into(),
        format!("saaa-session-{}", uuid::Uuid::new_v4()),
        receiver,
    )
    .await
    .err()
    .unwrap();
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
async fn backchannel_requires_context_window() {
    let (fake, server) = fixture().await;
    fake.omit_backchannel_window.store(true, Ordering::SeqCst);
    let value = fake.claim();
    assert_eq!(
        contract::parse(value, "session-1", &contract::required_providers()).err(),
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

fn machine_catalog() -> Value {
    let names = [
        ("llm", "ornith-1.5-35b", "llm.general", Some(131_072u64)),
        (
            "backchannel",
            "qwen3.5-2b-fast-response",
            "llm.backchannel.classifier",
            Some(65_536),
        ),
        ("asr", "qwen3-asr-1.7b", "speech.stt", None),
        ("tts", "voicevox-core", "speech.tts", None),
        ("embedding", "multilingual-e5-small", "embedding", None),
    ];
    let providers = names
        .iter()
        .map(|(name, model, capability, window)| {
            let mut provider = provider_decl(name, model, *window);
            provider["capability"] = json!(capability);
            provider["endpoint"] = json!(if *name == "embedding" {
                "/v1/embed"
            } else if *name == "llm" || *name == "backchannel" {
                "/v1/chat/completions"
            } else if *name == "asr" {
                "/v1/audio/transcriptions"
            } else {
                "/v1/audio/speech"
            });
            provider
        })
        .collect::<Vec<_>>();
    json!({
        "contractVersion": "agent-connection.v3",
        "catalogRevision": "rev-machine",
        "requestedProfile": "SAAA",
        "profiles": [{
            "id": "saaa-conversation-ornith15",
            "providers": providers,
            "services": []
        }]
    })
}

#[tokio::test]
async fn catalog_reads_llm_and_backchannel_details() {
    let (fake, server) = fixture().await;
    *fake.catalog.lock().unwrap() = Some(machine_catalog());
    let client = reqwest::Client::new();
    let base = url::Url::parse(&format!("{}/", fake.base)).unwrap();
    let profile = catalog::fetch(&client, &base, "test-control-token", "SAAA")
        .await
        .unwrap();
    let llm = profile.provider("llm").unwrap();
    assert_eq!(llm.model, "ornith-1.5-35b");
    assert_eq!(llm.capability, "llm.general");
    assert_eq!(llm.protocol, "openai.chat-completions.v1");
    assert_eq!(llm.endpoint, "/v1/chat/completions");
    let window = llm.context_window.unwrap();
    assert_eq!(
        (
            window.max_tokens,
            window.output_reserve_tokens,
            window.safety_margin_tokens
        ),
        (131_072, 4096, 1976)
    );
    let backchannel = profile.provider("backchannel").unwrap();
    assert_eq!(backchannel.model, "qwen3.5-2b-fast-response");
    assert_eq!(backchannel.capability, "llm.backchannel.classifier");
    assert_eq!(backchannel.protocol, "openai.chat-completions.v1");
    assert_eq!(backchannel.endpoint, "/v1/chat/completions");
    assert_eq!(backchannel.context_window.unwrap().max_tokens, 65_536);
    server.abort();
}

#[tokio::test]
async fn catalog_missing_model_is_invalid() {
    let (fake, server) = fixture().await;
    let mut response = machine_catalog();
    let backchannel = response["profiles"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .flat_map(|profile| profile["providers"].as_array_mut().unwrap())
        .find(|provider| provider["name"] == "backchannel")
        .unwrap();
    backchannel.as_object_mut().unwrap().remove("model");
    *fake.catalog.lock().unwrap() = Some(response);
    let client = reqwest::Client::new();
    let base = url::Url::parse(&format!("{}/", fake.base)).unwrap();
    let error = catalog::fetch(&client, &base, "test-control-token", "SAAA")
        .await
        .unwrap_err();
    assert_eq!(error, "larm_catalog_invalid");
    server.abort();
}

#[tokio::test]
async fn claim_model_mismatch_is_rejected_and_released() {
    let (fake, server) = fixture().await;
    *fake.llm_model.lock().unwrap() = Some("gemma4-e4b".into());
    let error = connect_variant(&fake, ProfileVariant::Conversation)
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "larm_create_claim_mismatch");
    assert_eq!(
        fake.log
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.starts_with("DELETE"))
            .count(),
        1
    );
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn claim_context_window_mismatch_is_rejected() {
    let (fake, server) = fixture().await;
    let mut response = catalog(&[("saaa-conversation-ornith15", FIVE)]);
    let backchannel = response["profiles"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .flat_map(|profile| profile["providers"].as_array_mut().unwrap())
        .find(|provider| provider["name"] == "backchannel")
        .unwrap();
    backchannel["contextWindow"]["maxTokens"] = json!(65_536);
    *fake.catalog.lock().unwrap() = Some(response);
    *fake.backchannel_max_tokens.lock().unwrap() = Some(230_400);
    let error = connect_variant(&fake, ProfileVariant::Conversation)
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "larm_catalog_claim_mismatch");
    server.abort();
}

#[tokio::test]
async fn renew_reclaim_is_verified_against_catalog() {
    let (fake, server) = fixture().await;
    let session = connect_variant(&fake, ProfileVariant::Conversation)
        .await
        .unwrap();
    *fake.llm_model.lock().unwrap() = Some("gemma4-e4b".into());
    session.snapshot.write().await.as_mut().unwrap().expires_at =
        chrono::Utc::now() + chrono::Duration::seconds(60);
    assert!(session.renew_if_due().await.is_err());
    assert!(session.acquire("llm").await.is_err());
    assert_eq!(fake.leases.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn provider_summary_exposes_models_without_tokens() {
    let (fake, server) = fixture().await;
    let session = connect_variant(&fake, ProfileVariant::Conversation)
        .await
        .unwrap();
    let summary = session.provider_summary().await;
    assert_eq!(summary.len(), 5);
    assert_eq!(session.selector(), Some("SAAA"));
    assert_eq!(session.catalog_revision(), Some("rev-fixture"));
    let rendered = format!("{summary:?}");
    assert!(!rendered.contains("token-"));
    assert!(!rendered.contains("Bearer"));
    assert!(summary.iter().any(|provider| {
        provider.name == "llm"
            && provider.model == "claimed-llm"
            && provider.endpoint == "/v1/chat/completions"
    }));
    assert!(summary.iter().any(|provider| {
        provider.name == "backchannel"
            && provider.context_window.unwrap().max_tokens == GEMMA4_KV_TOKENS
    }));
    session.close().await.unwrap();
    server.abort();
}
