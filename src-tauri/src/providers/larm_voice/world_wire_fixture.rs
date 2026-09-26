#![cfg(test)]
use super::*;
use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
    Json, Router,
};
use std::sync::{atomic::AtomicI64, Mutex as StdMutex};
pub(super) struct Fake {
    pub base: String,
    pub clock: Arc<AtomicI64>,
    pub bodies: StdMutex<Vec<Value>>,
    pub hits: StdMutex<Vec<String>>,
    pub catalog_queries: StdMutex<Vec<String>>,
    selected_profile: StdMutex<String>,
    pub released: AtomicBool,
    pub idle_released: AtomicBool,
    pub creates: std::sync::atomic::AtomicUsize,
    provider_idle_rejected: AtomicBool,
    route: &'static str,
    transition: &'static str,
    expires: String,
    created: String,
}
impl Fake {
    pub async fn start(
        route: &'static str,
        transition: &'static str,
        clock: Arc<AtomicI64>,
    ) -> (Arc<Self>, tokio::task::JoinHandle<()>) {
        Self::listen("127.0.0.1:0", route, transition, clock).await
    }

    pub async fn listen(
        address: &str,
        route: &'static str,
        transition: &'static str,
        clock: Arc<AtomicI64>,
    ) -> (Arc<Self>, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind(address).await.unwrap();
        let created = chrono::Utc::now() - chrono::Duration::seconds(1);
        let f = Arc::new(Self {
            base: format!("http://{}", listener.local_addr().unwrap()),
            clock,
            bodies: StdMutex::new(vec![]),
            hits: StdMutex::new(vec![]),
            catalog_queries: StdMutex::new(vec![]),
            selected_profile: StdMutex::new("SAAA".into()),
            released: AtomicBool::new(false),
            idle_released: AtomicBool::new(false),
            creates: std::sync::atomic::AtomicUsize::new(0),
            provider_idle_rejected: AtomicBool::new(false),
            route,
            transition,
            created: created.to_rfc3339(),
            expires: (created + chrono::Duration::seconds(300)).to_rfc3339(),
        });
        let router = Router::new().fallback(handle).with_state(f.clone());
        (
            f,
            tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            }),
        )
    }
    fn dynamic_claim(&self) -> Value {
        let url = url::Url::parse(&self.base).unwrap();
        let model = "ornith-1.5-35b";
        let endpoint = format!("{}/v1", self.base);
        json!({"id":"world-fixture","allocationId":"world-allocation","status":"ready","audience":"saaa-desktop","expiresAt":self.expires,"providers":[{"name":"llm","capability":"llm.general","apiStyle":"openai","protocol":"openai.chat-completions.v1","scheme":"http","host":"127.0.0.1","port":url.port().unwrap(),"baseUrl":endpoint,"model":model,"contextWindow":{"maxTokens":131072,"outputReserveTokens":4096,"safetyMarginTokens":1976},"health":{"url":format!("{}/v1/agent-connections/world-fixture/providers/llm/health",self.base),"kind":"semantic-inference","maxAgeMs":10000},"credential":{"type":"bearer","token":"token-llm","expiresAt":self.expires},"configuration":{"kind":"openai-provider-v1","fields":{"baseURL":endpoint,"model":model},"secretFields":{"apiKey":"credential.token"}}}]})
    }
    fn dynamic_state(&self, selector: &str) -> Value {
        let providers = ["tts", "llm", "embedding", "backchannel", "asr"]
            .into_iter()
            .map(|name| {
                let mut provider = declared_provider(name);
                provider["route"] = json!(name);
                provider["readiness"] = json!("ready");
                provider["claimable"] = json!(true);
                provider
            })
            .collect::<Vec<_>>();
        let services = match selector {
            "SAAA-w-Image" => vec![declared_service("image")],
            "SAAA-w-music" => vec![declared_service("music")],
            _ => vec![],
        };
        json!({"id":"world-fixture","allocationId":"world-allocation","bootEpoch":"epoch-fixture","catalogRevision":"0".repeat(64),"profile":selector,"agentProfile":"saaa-conversation-ornith15","profileRevision":"0".repeat(64),"audience":"saaa-desktop","audienceRevision":"0".repeat(64),"status":"ready","providers":providers,"services":services,"createdAt":self.created,"expiresAt":self.expires,"error":null})
    }
}
fn fixture_model(name: &str) -> &'static str {
    match name {
        "llm" => "ornith-1.5-35b",
        "backchannel" => "qwen3.5-2b-fast-response",
        "asr" => "qwen3-asr-1.7b",
        "tts" => "voicevox-core",
        "embedding" => "multilingual-e5-small",
        _ => "fixture",
    }
}

fn fixture_window(name: &str) -> Value {
    match name {
        "llm" => {
            json!({"maxTokens": 131072, "outputReserveTokens": 4096, "safetyMarginTokens": 1976})
        }
        "backchannel" => {
            json!({"maxTokens": 65536, "outputReserveTokens": 4096, "safetyMarginTokens": 1976})
        }
        _ => Value::Null,
    }
}

fn declared_provider(name: &str) -> Value {
    let (capability, protocol, endpoint) = match name {
        "llm" => (
            "llm.general",
            "openai.chat-completions.v1",
            "/v1/chat/completions",
        ),
        "backchannel" => (
            "llm.backchannel.classifier",
            "openai.chat-completions.v1",
            "/v1/chat/completions",
        ),
        "asr" => (
            "speech.stt",
            "openai.audio-transcriptions.v1",
            "/v1/audio/transcriptions",
        ),
        "tts" => ("speech.tts", "openai.audio-speech.v1", "/v1/audio/speech"),
        _ => ("embedding", "larm.embedding.v1", "/v1/embed"),
    };
    let mut value = json!({
        "name": name,
        "capability": capability,
        "protocol": protocol,
        "endpoint": endpoint,
        "model": fixture_model(name)
    });
    let window = fixture_window(name);
    if !window.is_null() {
        value["contextWindow"] = window;
    }
    value
}

fn declared_service(name: &str) -> Value {
    let (capability, protocol, endpoint, model) = match name {
        "image" => (
            "media.image.generate",
            "larm.image-generation.v1",
            "/v1/images/generations",
            "qwen-image-2.1",
        ),
        _ => (
            "media.music.generate",
            "larm.music-generation.v1",
            "/v1/music/generations",
            "ace-step-1.5",
        ),
    };
    json!({
        "name": name,
        "capability": capability,
        "protocol": protocol,
        "endpoint": endpoint,
        "model": model
    })
}

async fn handle(State(f): State<Arc<Fake>>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    if path == "/v3/agent-profiles" {
        let query = request.uri().query().unwrap_or("").to_string();
        f.catalog_queries.lock().unwrap().push(query.clone());
        if let Some(selector) = query.strip_prefix("profile=") {
            let services: &[&str] = match selector {
                "SAAA-w-Image" => &["image"],
                "SAAA-w-music" => &["music"],
                _ => &[],
            };
            let providers = ["asr", "backchannel", "embedding", "llm", "tts"]
                .iter()
                .map(|name| declared_provider(name))
                .collect::<Vec<_>>();
            let service_values = services
                .iter()
                .map(|name| declared_service(name))
                .collect::<Vec<_>>();
            return Json(json!({
                "contractVersion": "agent-connection.v3",
                "catalogRevision": "rev-fixture",
                "requestedProfile": selector,
                "profiles": [{
                    "id": "saaa-conversation-ornith15",
                    "providers": providers,
                    "services": service_values
                }]
            }))
            .into_response();
        }
        return ([("x-larm-config-revision","0000000000000000000000000000000000000000000000000000000000000000")],Json(json!({"contractVersion":"agent-connection.v1","profiles":[{"id":"saaa-qwen38","providers":[{"name":"llm","capability":"llm.reasoning","protocol":"openai.chat-completions.v1","model":"qwen3.8-27b","contextWindow":{"maxTokens":230400,"outputReserveTokens":4096,"safetyMarginTokens":1976}}]}],"audiences":["saaa-desktop"]}))).into_response();
    }
    if path == "/v1/agent-connections/world-fixture/providers/llm/health" {
        assert_eq!(request.headers()["authorization"], "Bearer token-llm");
        return Json(json!({"ready":true,"acceptingRequests":true,"capacity":{"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":1,"queueDepth":0,"queueTimeoutMs":1000,"retryAfterMs":0,"completionGuaranteed":false},"probe":{"validated":true,"protocol":"openai.chat-completions.v1"}})).into_response();
    }
    if path.starts_with("/v1/agent-connections") {
        assert!(request.headers().get("authorization").is_some());
        if request.method() == "DELETE" {
            f.released.store(true, Ordering::SeqCst);
            f.idle_released.store(false, Ordering::SeqCst);
            return axum::http::StatusCode::NO_CONTENT.into_response();
        }
        if path.ends_with("/claim") {
            if f.route == "dynamic-lan" {
                return Json(f.dynamic_claim()).into_response();
            }
            let mut names = vec![
                ("llm", "openai.chat-completions.v1"),
                ("tts", "openai.audio-speech.v1"),
                ("embedding", "larm.embedding.v1"),
                ("asr", "openai.audio-transcriptions.v1"),
            ];
            names.push(("backchannel", "openai.chat-completions.v1"));
            let providers = names
                .iter()
                .map(|(name, protocol)| {
                    json!({
                        "name": name,
                        "protocol": protocol,
                        "baseUrl": format!("{}/{name}/v1", f.base),
                        "model": fixture_model(name),
                        "configuration": {"fields": if *name == "embedding" {
                            json!({"daemonURL":format!("{}/{name}/v1", f.base),"model":fixture_model(name),"dimension":384})
                        } else {
                            json!({"baseURL":format!("{}/{name}/v1", f.base),"model":fixture_model(name)})
                        }},
                        "credential": {"token": format!("token-{name}")},
                        "health": {"url": format!("{}/{name}/health", f.base), "maxAgeMs": 10000},
                        "contextWindow": fixture_window(name),
                        "embeddingSpace": if *name == "embedding" { json!({"dimension": 384}) } else { Value::Null }
                    })
                })
                .collect::<Vec<_>>();
            return Json(json!({
                "id": "world-fixture",
                "status": "ready",
                "allocationId": "world-allocation",
                "expiresAt": f.expires,
                "providers": providers
            }))
            .into_response();
        }
        let is_create = request.method() == axum::http::Method::POST;
        if is_create {
            f.creates.fetch_add(1, Ordering::SeqCst);
        }
        if f.transition == "initial" && is_create {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        if is_create {
            f.clock.fetch_add(3000, Ordering::SeqCst);
        }
        let requested_profile = if is_create {
            let bytes = axum::body::to_bytes(request.into_body(), 65536)
                .await
                .unwrap();
            serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|body| body["profile"].as_str().map(str::to_string))
                .unwrap_or_else(|| "SAAA".into())
        } else {
            f.selected_profile.lock().unwrap().clone()
        };
        if is_create {
            *f.selected_profile.lock().unwrap() = requested_profile.clone();
        }
        let mut value = if f.route == "dynamic-lan" {
            f.dynamic_state(&requested_profile)
        } else {
            {
                let providers = ["tts", "llm", "embedding", "backchannel", "asr"]
                    .into_iter()
                    .map(|name| {
                        let mut provider = declared_provider(name);
                        provider["readiness"] = json!("ready");
                        provider["claimable"] = json!(true);
                        provider
                    })
                    .collect::<Vec<_>>();
                let services = match requested_profile.as_str() {
                    "SAAA-w-Image" => vec![declared_service("image")],
                    "SAAA-w-music" => vec![declared_service("music")],
                    _ => vec![],
                };
                json!({"id":"world-fixture","profile":requested_profile,"agentProfile":"saaa-conversation-ornith15","status":"ready","allocationId":"world-allocation","expiresAt":f.expires,"providers":providers,"services":services})
            }
        };
        if !is_create && f.idle_released.load(Ordering::SeqCst) {
            value["status"] = json!("released");
            value["reason"] = json!("foreground_idle_timeout");
        }
        return (
            if is_create {
                axum::http::StatusCode::CREATED
            } else {
                axum::http::StatusCode::OK
            },
            Json(value),
        )
            .into_response();
    }
    let name = if f.route == "dynamic-lan" {
        "llm"
    } else {
        path.split('/').nth(1).unwrap()
    };
    if f.route != "openai-compatible" {
        assert_eq!(
            request.headers()["authorization"],
            format!("Bearer token-{name}")
        );
    }
    if f.transition == "idle-provider-reject"
        && name == "llm"
        && !f.provider_idle_rejected.swap(true, Ordering::SeqCst)
    {
        f.idle_released.store(true, Ordering::SeqCst);
        return (
            axum::http::StatusCode::CONFLICT,
            Json(json!({"error":{"code":"connection_idle_released"}})),
        )
            .into_response();
    }
    if path.ends_with("/health") {
        let protocol = match name {
            "asr" => "openai.audio-transcriptions.v1",
            "tts" => "openai.audio-speech.v1",
            "embedding" => "larm.embedding.v1",
            _ => "openai.chat-completions.v1",
        };
        return Json(json!({"ready":true,"acceptingRequests":true,"capacity":{"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":1,"queueDepth":0,"queueTimeoutMs":1000,"retryAfterMs":0,"completionGuaranteed":false},"probe":{"validated":true,"protocol":protocol}})).into_response();
    }
    assert!(matches!(
        path.as_str(),
        "/llm/v1/chat/completions" | "/backchannel/v1/chat/completions" | "/v1/chat/completions"
    ));
    if f.route == "butler" && name == "backchannel" && f.transition == "frontend-slow" {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    if f.route == "butler" && name == "llm" && f.transition == "reasoner-slow" {
        tokio::time::sleep(std::time::Duration::from_secs(25)).await;
    }
    if f.route == "butler"
        && name == "llm"
        && f.transition == "reasoner-fifteen"
        && !f.hits.lock().unwrap().iter().any(|hit| hit == "llm")
    {
        tokio::time::sleep(std::time::Duration::from_secs(13)).await;
    }
    if f.route == "butler" && name == "llm" && f.transition == "reasoner-hang" {
        tokio::time::sleep(std::time::Duration::from_secs(180)).await;
    }
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(request.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    let count = {
        let mut bodies = f.bodies.lock().unwrap();
        bodies.push(body.clone());
        f.hits.lock().unwrap().push(name.to_string());
        bodies.len()
    };
    if f.route == "butler" && name == "backchannel" {
        let content = match f.transition {
            "frontend-bad" => "not-json",
            "frontend-nod" => r#"{"kind":"nod","reply":"x"}"#,
            "frontend-thanks" => r#"{"kind":"thanks","reply":"x"}"#,
            "frontend-greeting" => r#"{"kind":"greeting","reply":"x"}"#,
            "frontend-low" => r#"{"kind":"handoff","reply":"少し考えます。"}"#,
            _ => r#"{"kind":"handoff","reply":"少し考えます。"}"#,
        };
        return Json(json!({"choices":[{"message":{"content":content}}]})).into_response();
    }
    let streaming = body["stream"] == true;
    // Chat completions retries a transient 503 twice inside one provider attempt. Keep all
    // three transport requests unavailable so the caller reaches the provider-fallback
    // boundary; the next provider attempt succeeds with a newly prepared World frame.
    if f.transition == "fallback" && count <= 3 {
        f.clock.fetch_add(400, Ordering::SeqCst);
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let message = if f.transition == "context-still" && count == 1 {
        json!({"role":"assistant","content":null,"tool_calls":[{"id":"context-still-tool","type":"function","function":{"name":crate::memory::context_still_search::SEARCH_KNOWLEDGE_TOOL_NAME,"arguments":json!({"query":"SAAA LARM provider descriptor context window","limit":1}).to_string()}}]})
    } else if f.transition == "tool-continuation" && count == 1 {
        f.clock.fetch_add(3000, Ordering::SeqCst);
        json!({"role":"assistant","content":null,"tool_calls":[{"id":"world-tool","type":"function","function":{"name":"present_ui","arguments":json!({"definition":"root=ModelStatus(\"larm.status\")","summary":"fixture","mode":"live"}).to_string()}}]})
    } else {
        json!({"role":"assistant","content":if f.transition=="context-still" {"ContextStillの検索結果を確認しました。"} else if f.route=="butler" {"ornith-answer"} else {"fixture"}})
    };
    let finish = if message.get("tool_calls").is_some() {
        "tool_calls"
    } else {
        "stop"
    };
    if !streaming {
        return Json(json!({"choices":[{"index":0,"message":message,"finish_reason":finish}]}))
            .into_response();
    }
    let mut delta = message;
    if let Some(calls) = delta["tool_calls"].as_array_mut() {
        for (i, c) in calls.iter_mut().enumerate() {
            c["index"] = json!(i);
        }
    }
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
        ),
    )
        .into_response()
}
