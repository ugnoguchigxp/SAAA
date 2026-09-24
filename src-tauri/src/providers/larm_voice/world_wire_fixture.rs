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
    pub released: AtomicBool,
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
            released: AtomicBool::new(false),
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
        let model = "qwen3.8-27b";
        let endpoint = format!("{}/v1", self.base);
        json!({"id":"world-fixture","allocationId":"world-allocation","status":"ready","audience":"saaa-desktop","expiresAt":self.expires,"providers":[{"name":"llm","capability":"llm.reasoning","apiStyle":"openai","protocol":"openai.chat-completions.v1","scheme":"http","host":"127.0.0.1","port":url.port().unwrap(),"baseUrl":endpoint,"model":model,"health":{"url":format!("{}/v1/agent-connections/world-fixture/providers/llm/health",self.base),"kind":"semantic-inference","maxAgeMs":10000},"credential":{"type":"bearer","token":"token-llm","expiresAt":self.expires},"configuration":{"kind":"openai-provider-v1","fields":{"baseURL":endpoint,"model":model},"secretFields":{"apiKey":"credential.token"}}}]})
    }
    fn dynamic_state(&self) -> Value {
        json!({"id":"world-fixture","allocationId":"world-allocation","bootEpoch":"epoch-fixture","catalogRevision":"0".repeat(64),"agentProfile":"saaa-qwen38","profileRevision":"0".repeat(64),"audience":"saaa-desktop","audienceRevision":"0".repeat(64),"status":"ready","providers":[{"name":"llm","capability":"llm.reasoning","route":"llm","protocol":"openai.chat-completions.v1","publicModel":"qwen3.8-27b","readiness":"ready","claimable":true}],"createdAt":self.created,"expiresAt":self.expires,"error":null})
    }
}
async fn handle(State(f): State<Arc<Fake>>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    if path == "/v3/agent-profiles" {
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
            if f.route == "butler" {
                names.push(("backchannel", "openai.chat-completions.v1"));
            }
            return Json(json!({"id":"world-fixture","status":"ready","allocationId":"world-allocation","expiresAt":f.expires,"providers":(names.iter().map(|(name,protocol)|json!({"name":name,"protocol":protocol,"baseUrl":format!("{}/{name}/v1",f.base),"model":if *name=="llm" {"gemma-4-e4b"} else if *name=="embedding" {"multilingual-e5-small"} else {"fixture"},"configuration":{"fields":{}},"credential":{"token":format!("token-{name}")},"health":{"url":format!("{}/{name}/health",f.base),"maxAgeMs":10000},"contextWindow":if *name=="llm" || *name=="backchannel" {json!({"maxTokens":230400,"outputReserveTokens":4096,"safetyMarginTokens":1976})} else {Value::Null},"embeddingSpace":if *name=="embedding" {json!({"dimension":384})} else {Value::Null}})).collect::<Vec<_>>())})).into_response();
        }
        if f.transition == "initial" {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
        f.clock.fetch_add(3000, Ordering::SeqCst);
        let value = if f.route == "dynamic-lan" {
            f.dynamic_state()
        } else {
            json!({"id":"world-fixture","status":"ready","allocationId":"world-allocation","expiresAt":f.expires})
        };
        return (axum::http::StatusCode::CREATED, Json(value)).into_response();
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
        "/llm/v1/chat/completions"
            | "/backchannel/v1/chat/completions"
            | "/v1/chat/completions"
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
        tokio::time::sleep(std::time::Duration::from_secs(120)).await;
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
            "frontend-nod" => {
                r#"{"ack":"nod","intent":"acknowledgement","resolvesTurn":true,"confidence":"high"}"#
            }
            "frontend-thanks" => {
                r#"{"ack":"thanks","intent":"social","resolvesTurn":true,"confidence":"high"}"#
            }
            "frontend-low" => {
                r#"{"ack":"nod","intent":"acknowledgement","resolvesTurn":true,"confidence":"low"}"#
            }
            _ => r#"{"ack":"working","intent":"request","resolvesTurn":false,"confidence":"high"}"#,
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
