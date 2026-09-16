use axum::{
    routing::{delete, get, post},
    Json, Router,
};
use saaa_larm_session::contexts::*;
use serde_json::json;
fn request() -> ViewRequest {
    ViewRequest {
        allocation_id: "a".into(),
        runtime: "r".into(),
        base_input_tokens: 10,
        max_input_tokens: 100,
        deadline: "2099-01-01T00:00:00Z".into(),
        canonicalization_version: "context-view-v1".into(),
        items: vec![PlanItem {
            context_id: "c".into(),
            version: "1".into(),
            required: true,
            utility: 1.0,
        }],
    }
}
fn view() -> View {
    serde_json::from_value(json!({"schemaVersion":1,"id":"v","operationId":"o","allocationId":"a","runtime":"r","release":"release","compatibilityKey":"a".repeat(64),"viewDigest":"b".repeat(64),"canonicalizationVersion":"context-view-v1","baseInputTokens":10,"inputBudgetTokens":100,"tokenCount":20,"orderedItems":[{"contextId":"c","version":"1","required":true,"utility":1.0,"tokenCount":10,"sourceDigest":"c".repeat(64)}],"omitted":[],"leaseEpoch":1,"state":"ready","createdAt":"2026-01-01T00:00:00Z","expiresAt":"2099-01-01T00:00:00Z"})).unwrap()
}
#[test]
fn bindings_required_and_replay_states_fail_closed() {
    assert!(view().validate(&request(), "release", 1, 1).is_ok());
    for state in ["consumed", "expired", "invalid"] {
        let mut v = view();
        v.state = state.into();
        assert!(v.validate(&request(), "release", 1, 1).is_err());
    }
    assert!(view().validate(&request(), "new-release", 1, 1).is_err());
    assert!(view().validate(&request(), "release", 2, 1).is_err());
    let mut v = view();
    v.ordered_items.clear();
    v.omitted = vec![Omission {
        context_id: "c".into(),
        version: "1".into(),
        reason: "budget".into(),
    }];
    assert!(v.validate(&request(), "release", 1, 1).is_err());
}
async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
    (format!("http://{address}"), task)
}
#[tokio::test]
async fn context_off_delete_success_does_not_mean_absent() {
    let app = Router::new()
        .route(
            "/v1/contexts/{id}",
            delete(|| async { axum::http::StatusCode::NO_CONTENT }),
        )
        .route(
            "/v1/contexts",
            get(|| async { Json(json!({"contexts":[{"id":"c","state":"active"}]})) }),
        );
    let (url, server) = serve(app).await;
    let client = Client::new(&url, "fixture-token".into()).unwrap();
    client.delete_registration("c", "delete-1").await.unwrap();
    assert!(!client.registration_absent("c").await.unwrap());
    server.abort();
}
#[tokio::test]
async fn managed_headers_and_pinned_view_request_reach_http() {
    let app = Router::new()
        .route(
            "/v1/context-views",
            post(
                |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| async move {
                    assert_eq!(headers["idempotency-key"], "vkey");
                    assert_eq!(headers["authorization"], "Bearer fixture-token");
                    assert_eq!(body["canonicalizationVersion"], "context-view-v1");
                    Json(view())
                },
            ),
        )
        .route(
            "/v1/chat/completions",
            post(|headers: axum::http::HeaderMap| async move {
                assert_eq!(headers["x-larm-allocation-id"], "a");
                assert_eq!(headers["x-larm-context-view-id"], "v");
                assert_eq!(headers["x-larm-capability"], "llm.coding");
                Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"fixture"}}]}))
            }),
        );
    let (url, server) = serve(app).await;
    let client = Client::new(&url, "fixture-token".into()).unwrap();
    let v = client.create_view(&request(), "vkey").await.unwrap();
    v.validate(&request(), "release", 1, 1).unwrap();
    let answer = client
        .chat(&v, "llm.coding", &json!({"stream":false}))
        .await
        .unwrap();
    assert_eq!(answer["choices"][0]["message"]["content"], "fixture");
    server.abort();
}
#[tokio::test]
async fn illegal_paths_and_quotas_are_rejected_without_io() {
    let client = Client::new("http://127.0.0.1:1", "fixture".into()).unwrap();
    assert!(client
        .delete_registration("../other", "x")
        .await
        .unwrap_err()
        .contains("context-id"));
    let r = Registration {
        id: "c".into(),
        version: "1".into(),
        source_handle: "h".into(),
        source_digest: "a".repeat(64),
        classification: "confidential".into(),
        byte_count: 20,
        token_count: 20_000_001,
        tokenizer_digest: "b".repeat(64),
        expires_at: None,
    };
    assert_eq!(
        client.register(&r, "x").await.unwrap_err(),
        "context-registration-invalid"
    );
}
