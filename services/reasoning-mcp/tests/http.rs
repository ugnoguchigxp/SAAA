use axum::{routing::post, Json, Router};
use saaa_reasoning_contract::{Request, Response, PROTOCOL};
use saaa_reasoning_mcp::{provider::Provider, Service};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;
const TOKEN: &str = "fixture-bearer-not-a-secret";
fn request() -> Value {
    serde_json::from_str(include_str!(
        "../../../crates/reasoning-contract/fixtures/request.json"
    ))
    .unwrap()
}
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, task)
}
fn rpc_post(client: &reqwest::Client, url: &str, body: Value) -> reqwest::RequestBuilder {
    client
        .post(format!("{url}/mcp"))
        .bearer_auth(TOKEN)
        .header("MCP-Protocol-Version", PROTOCOL)
        .header("Accept", "application/json, text/event-stream")
        .json(&body)
}
fn call(id: &str) -> Value {
    let mut r = request();
    r["requestId"] = id.into();
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"reasoning.answer","arguments":r}})
}
#[tokio::test]
async fn real_http_handshake_answer_and_bad_provider_terminal() {
    let upstream = Router::new().route("/complete", post_handler());
    let (provider_url, upstream_task) = serve(upstream).await;
    let service = Service::new(
        Provider::new(
            &format!("{provider_url}/complete"),
            "fixture-model-from-provider".into(),
            None,
        )
        .unwrap(),
        TOKEN.into(),
    )
    .unwrap();
    let (url, server) = serve(service.router()).await;
    let client = reqwest::Client::new();
    let init:Value=rpc_post(&client,&url,json!({"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":PROTOCOL}})).send().await.unwrap().json().await.unwrap();
    assert_eq!(init["result"]["protocolVersion"], PROTOCOL);
    assert_eq!(
        rpc_post(
            &client,
            &url,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .send()
        .await
        .unwrap()
        .status(),
        202
    );
    let list: Value = rpc_post(
        &client,
        &url,
        json!({"jsonrpc":"2.0","id":"list","method":"tools/list"}),
    )
    .send()
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(list["result"]["tools"][0]["name"], "reasoning.answer");
    let value: Value = rpc_post(&client, &url, call("request_fixture"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let answer: Response =
        serde_json::from_value(value["result"]["structuredContent"].clone()).unwrap();
    answer
        .validate(&serde_json::from_value::<Request>(request()).unwrap())
        .unwrap();
    assert_eq!(answer.speech_text, "条件を確認しました。");
    assert_eq!(
        client
            .post(format!("{url}/mcp"))
            .json(&call("unauthorized"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        rpc_post(&client, &url, call("origin"))
            .header("Origin", "https://example.com")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let mut bad = call("bad");
    bad["params"]["arguments"]["constraints"]["localOnly"] = false.into();
    let value: Value = rpc_post(&client, &url, bad)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["error"]["code"], -32602);
    server.abort();
    upstream_task.abort();
}
fn post_handler() -> axum::routing::MethodRouter {
    post(|Json(body): Json<Value>| async move {
        assert_eq!(body["model"], "fixture-model-from-provider");
        assert_eq!(body["stream"], false);
        Json(
            json!({"choices":[{"finish_reason":"stop","message":{"content":json!({"intent":"answer","speechText":"条件を確認しました。","keyPoints":[],"evidenceIds":[],"limitations":[]}).to_string()}}]}),
        )
    })
}
#[tokio::test]
async fn cancellation_frees_capacity_and_never_returns_answer() {
    let entered = Arc::new(Notify::new());
    let signal = entered.clone();
    let upstream = Router::new().route(
        "/complete",
        post(move || {
            let signal = signal.clone();
            async move {
                signal.notify_one();
                tokio::time::sleep(Duration::from_secs(30)).await;
                Json(json!({}))
            }
        }),
    );
    let (provider_url, upstream_task) = serve(upstream).await;
    let service = Service::new(
        Provider::new(&format!("{provider_url}/complete"), "test".into(), None).unwrap(),
        TOKEN.into(),
    )
    .unwrap();
    let (url, server) = serve(service.router()).await;
    let client = reqwest::Client::new();
    let first = tokio::spawn(rpc_post(&client, &url, call("cancel_me")).send());
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    let busy: Value = rpc_post(&client, &url, call("busy"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(busy["result"]["content"][0]["text"], "busy");
    rpc_post(&client,&url,json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"cancel_me"}})).send().await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(1), first)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["result"]["isError"], true);
    assert_eq!(value["result"]["content"][0]["text"], "cancelled");
    let mut short = call("timeout");
    short["params"]["arguments"]["budget"]["timeoutMs"] = 10.into();
    let value: Value = rpc_post(&client, &url, short)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["result"]["content"][0]["text"], "timeout");
    server.abort();
    upstream_task.abort();
}
#[tokio::test]
async fn truncated_or_fabricated_output_is_not_an_answer() {
    for (finish, content) in [
        (
            "length",
            json!({"intent":"answer","speechText":"未完了","keyPoints":[],"evidenceIds":[],"limitations":[]}),
        ),
        (
            "stop",
            json!({"intent":"answer","speechText":"捏造根拠","keyPoints":[],"evidenceIds":["missing"],"limitations":[]}),
        ),
    ] {
        let upstream=Router::new().route("/complete",post(move || {let content=content.clone();async move {Json(json!({"choices":[{"finish_reason":finish,"message":{"content":content.to_string()}}]}))}}));
        let (provider_url, upstream_task) = serve(upstream).await;
        let service = Service::new(
            Provider::new(&format!("{provider_url}/complete"), "test".into(), None).unwrap(),
            TOKEN.into(),
        )
        .unwrap();
        let (url, server) = serve(service.router()).await;
        let value: Value = rpc_post(&reqwest::Client::new(), &url, call("request_fixture"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(value["result"]["isError"], true);
        assert!(value["result"].get("structuredContent").is_none());
        server.abort();
        upstream_task.abort();
    }
}

#[tokio::test]
async fn cancellation_arriving_before_call_prevents_generation() {
    let provider =
        Provider::new("http://127.0.0.1:1/complete", "unused-model".into(), None).unwrap();
    let (url, server) = serve(Service::new(provider, TOKEN.into()).unwrap().router()).await;
    let client = reqwest::Client::new();
    rpc_post(
        &client,
        &url,
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"early"}}),
    )
    .send()
    .await
    .unwrap();
    let value: Value = rpc_post(&client, &url, call("early"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["result"]["content"][0]["text"], "cancelled");
    server.abort();
}
