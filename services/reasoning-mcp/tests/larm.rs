use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
struct Fake {
    base: String,
    called: AtomicBool,
    released: AtomicBool,
}
async fn larm(State(fake): State<Arc<Fake>>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let expires = chrono_expiry();
    if path.starts_with("/v1/agent-connections") {
        assert_eq!(
            request.headers()["authorization"],
            "Bearer test-control-token"
        );
        if request.method() == "DELETE" {
            fake.released.store(true, Ordering::SeqCst);
            return axum::http::StatusCode::NO_CONTENT.into_response();
        }
        let mut value = json!({"id":"fixture","status":"ready","allocationId":"allocation-fixture","expiresAt":expires});
        if path.ends_with("/claim") {
            value["providers"]=json!([
                ("llm","openai.chat-completions.v1"),("tts","openai.audio-speech.v1"),
                ("backchannel","openai.chat-completions.v1"),("asr","openai.audio-transcriptions.v1")
            ].iter().map(|(name,protocol)|json!({"name":name,"protocol":protocol,
                "configuration":{"fields":{"baseURL":format!("{}/{name}/v1",fake.base),"model":format!("{name}-from-claim")}},
                "credential":{"token":format!("{name}-exclusive-token")},"health":{"url":format!("{}/{name}/health",fake.base),"maxAgeMs":10000},
                "contextWindow":if *protocol=="openai.chat-completions.v1" {json!({"maxTokens":65536,"outputReserveTokens":4096,"safetyMarginTokens":1024})} else {Value::Null}})).collect::<Vec<_>>());
            return Json(value).into_response();
        }
        return (axum::http::StatusCode::CREATED, Json(value)).into_response();
    }
    let name = path.split('/').nth(1).unwrap();
    assert_eq!(
        request.headers()["authorization"],
        format!("Bearer {name}-exclusive-token")
    );
    if path.ends_with("/health") {
        let protocol = match name {
            "asr" => "openai.audio-transcriptions.v1",
            "tts" => "openai.audio-speech.v1",
            _ => "openai.chat-completions.v1",
        };
        return Json(json!({"ready":true,"acceptingRequests":true,"capacity":{"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":1,"queueDepth":0,"queueTimeoutMs":1000,"retryAfterMs":0,"completionGuaranteed":false},"probe":{"validated":true,"protocol":protocol}})).into_response();
    }
    assert_eq!(path, "/llm/v1/chat/completions");
    assert!(request.headers().get("x-larm-context-view-id").is_none());
    let bytes = axum::body::to_bytes(request.into_body(), 65536)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["model"], "llm-from-claim");
    fake.called.store(true, Ordering::SeqCst);
    Json(json!({"choices":[{"finish_reason":"stop","message":{"content":json!({"intent":"answer","speechText":"条件を確認しました。","keyPoints":[],"evidenceIds":[],"limitations":[]}).to_string()}}]})).into_response()
}
fn chrono_expiry() -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(600)).to_rfc3339()
}
#[tokio::test]
async fn mcp_uses_the_session_claim_and_does_not_invent_context_headers() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fake = Arc::new(Fake {
        base: format!("http://{}", listener.local_addr().unwrap()),
        called: AtomicBool::new(false),
        released: AtomicBool::new(false),
    });
    let router = Router::new().fallback(larm).with_state(fake.clone());
    let upstream = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (_stop, receiver) = tokio::sync::watch::channel(false);
    let session = saaa_larm_session::Session::connect_with_profile_and_credential(
        &fake.base,
        saaa_larm_session::DEFAULT_PROFILE,
        "test-control-token".into(),
        receiver,
    )
    .await
    .unwrap();
    let provider = saaa_reasoning_mcp::provider::Provider::from_larm(session.clone()).unwrap();
    let service =
        saaa_reasoning_mcp::Service::new(provider, "fixture-mcp-exclusive-token".into()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, service.router()).await.unwrap();
    });
    let request: Value = serde_json::from_str(include_str!(
        "../../../crates/reasoning-contract/fixtures/request.json"
    ))
    .unwrap();
    let response:Value=reqwest::Client::new().post(endpoint).bearer_auth("fixture-mcp-exclusive-token")
        .header("MCP-Protocol-Version",saaa_reasoning_contract::PROTOCOL)
        .json(&json!({"jsonrpc":"2.0","id":request["requestId"],"method":"tools/call","params":{"name":"reasoning.answer","arguments":request}}))
        .send().await.unwrap().json().await.unwrap();
    session.close().await.unwrap();
    assert_eq!(
        response["result"]["structuredContent"]["speechText"],
        "条件を確認しました。"
    );
    assert!(fake.called.load(Ordering::SeqCst));
    assert!(fake.released.load(Ordering::SeqCst));
    server.abort();
    upstream.abort();
}
