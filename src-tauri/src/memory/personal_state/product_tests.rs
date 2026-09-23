#![cfg(test)]
use super::*;
use crate::{database_error, persistence::sqlite::SqliteWriter, RunCancellation};
use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
    Json, Router,
};
use saaa_larm_session::{contexts::Client, personal_state::Capability};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
#[derive(Clone)]
struct Host {
    pub(super) cap: Capability,
    pub(super) log: Arc<Mutex<Vec<String>>>,
    pub(super) state: Arc<Mutex<std::collections::BTreeMap<String, Value>>>,
    pub(super) partial: bool,
    pub(super) wrong_subject: bool,
    pub(super) lost_source: bool,
    pub(super) forget_writer: Arc<Mutex<Option<Arc<SqliteWriter>>>>,
    pub(super) pause_source: Arc<std::sync::atomic::AtomicBool>,
    pub(super) source_arrived: Arc<tokio::sync::Notify>,
}
async fn http(State(h): State<Host>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let method = req.method().to_string();
    assert_eq!(req.headers()["authorization"], "Bearer fixture-secret");
    h.log.lock().unwrap().push(format!("{method} {path}"));
    let headers = req.headers().clone();
    let bytes = axum::body::to_bytes(req.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let cap = &h.cap;
    let common = json!({"contractVersion":"larm-personal-state.v1","subjectDigest":if h.wrong_subject {"f".repeat(64)}else{cap.subject_digest.clone()},"allocationId":cap.allocation_id,"runtime":cap.runtime,"release":cap.release,"leaseEpoch":cap.lease_epoch,"dataEpoch":0,"tokenizerDigest":cap.tokenizer_digest,"chatTemplateDigest":cap.chat_template_digest,"expiresAt":"2099-01-01T00:00:00Z"});
    let mut v = common;
    if method == "GET" {
        return Json(
            h.state
                .lock()
                .unwrap()
                .get(&path)
                .cloned()
                .unwrap_or(json!({})),
        )
        .into_response();
    }
    if path == "/v1/context-sources" {
        let id = headers["x-larm-source-incarnation"].to_str().unwrap();
        v["incarnation"] = json!(id);
        v["sourceHandle"] = json!(format!("handle-{id}"));
        v["sourceDigest"] = json!(headers["x-larm-source-digest"].to_str().unwrap());
        v["byteCount"] = json!(bytes.len());
        v["tokenCount"] = json!(10);
        v["state"] = json!("succeeded");
        h.state
            .lock()
            .unwrap()
            .insert(format!("/v1/context-source-operations/{id}"), v.clone());
        if let Some(writer) = h.forget_writer.lock().unwrap().take() {
            writer
                .write(|c| {
                    c.execute("DELETE FROM conversation_messages WHERE id='s1'", [])
                        .map_err(database_error)?;
                    Ok(())
                })
                .unwrap();
        }
        h.source_arrived.notify_one();
        while h.pause_source.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        if h.lost_source {
            return axum::http::StatusCode::BAD_GATEWAY.into_response();
        }
    } else {
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
        match path.as_str() {
            "/v1/contexts" => {
                v = body;
                v["state"] = json!("active");
                h.state
                    .lock()
                    .unwrap()
                    .insert(v["id"].as_str().unwrap().into(), v.clone());
            }
            "/v1/context-measurements" => {
                assert!(body["request"]["messages"].is_array());
                v["measurementId"] = body["measurementId"].clone();
                v["requestDigest"] = json!("d".repeat(64));
                v["baseInputTokens"] = json!(10);
                v["maxInputTokens"] = body["maxInputTokens"].clone();
            }
            "/v2/context-views" => {
                assert_eq!(body["canonicalizationVersion"], "context-view-v2");
                let id = body["viewRequestId"].as_str().unwrap();
                let receipt = json!({"subjectDigest":cap.subject_digest,"bootEpoch":cap.boot_epoch,"viewId":id,"requestDigest":"d".repeat(64),"state":"ready"});
                h.state
                    .lock()
                    .unwrap()
                    .insert(format!("/v2/context-views/{id}"), receipt);
                let items=body["items"].as_array().unwrap().iter().map(|i|{let r=h.state.lock().unwrap()[i["contextId"].as_str().unwrap()].clone();json!({"contextId":i["contextId"],"version":i["version"],"required":true,"utility":1.0,"tokenCount":10,"sourceDigest":r["sourceDigest"]})}).collect::<Vec<_>>();
                v = json!({"schemaVersion":1,"id":id,"operationId":"op","allocationId":cap.allocation_id,"runtime":cap.runtime,"release":cap.release,"compatibilityKey":"b".repeat(64),"viewDigest":"c".repeat(64),"canonicalizationVersion":"context-view-v2","baseInputTokens":10,"inputBudgetTokens":1000,"tokenCount":20,"orderedItems":items,"omitted":[],"leaseEpoch":1,"requestDigest":"d".repeat(64),"dataEpoch":0,"state":"ready","createdAt":"2026-01-01T00:00:00Z","expiresAt":"2099-01-01T00:00:00Z"});
            }
            "/v1/chat/completions" => {
                let id = headers["x-larm-attempt-id"].to_str().unwrap();
                v["attemptId"] = json!(id);
                v["state"] = json!("completed");
                v["requestDigest"] = json!("d".repeat(64));
                h.state
                    .lock()
                    .unwrap()
                    .insert(format!("/v1/generation-attempts/{id}"), v);
                v = json!({"choices":[{"finish_reason":"stop","message":{"content":"{\"candidates\":[],\"no_change\":true}"}}]});
            }
            "/v1/context-forget-operations" => {
                let id = body["forgetId"].as_str().unwrap();
                v = json!({"contractVersion":"larm-personal-state.v1","subjectDigest":cap.subject_digest,"forgetId":id,"state":"succeeded","absenceVerified":true,"phases":{}});
                for phase in [
                    "attempts",
                    "views",
                    "runtime",
                    "snapshots",
                    "registry",
                    "sources",
                    "audit",
                ] {
                    v["phases"][phase] = json!({"state":if h.partial && phase=="snapshots" {"pending"}else{"absent"}});
                }
                h.state
                    .lock()
                    .unwrap()
                    .insert(format!("/v1/context-forget-operations/{id}"), v.clone());
            }
            _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
        }
    }
    Json(v).into_response()
}
async fn fixture(
    partial: bool,
    wrong_subject: bool,
    lost_source: bool,
) -> (
    managed::Adapter,
    generation::Manifest,
    Vec<saaa_personal_state_core::SourceRef>,
    Host,
    tokio::task::JoinHandle<()>,
) {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    crate::initialize_database(&c).unwrap();
    c.execute(
        "INSERT INTO conversation_messages VALUES('s1',?1,'user','公開は保留する','1')",
        [crate::PRIMARY_CONVERSATION_ID],
    )
    .unwrap();
    let source = sources::load(&c, 1, 0, 4096).unwrap().source;
    let l = store::load(&c).unwrap();
    let cap = Capability {
        contract_version: "larm-personal-state.v1".into(),
        boot_epoch: uuid::Uuid::new_v4().to_string(),
        subject_digest: "a".repeat(64),
        allocation_id: "allocation".into(),
        runtime: "runtime".into(),
        release: "release".into(),
        lease_epoch: 1,
        lease_expires_at: "2099-01-01T00:00:00Z".into(),
        credential_expires_at: "2099-01-01T00:00:00Z".into(),
        tokenizer_digest: "b".repeat(64),
        chat_template_digest: "c".repeat(64),
        context_limit_tokens: 4000,
        output_reserve_tokens: 2000,
        safety_margin_tokens: 1000,
        source_token_limit: 20_000_000,
        max_source_bytes: 262144,
        max_total_source_bytes: 1048576,
        max_materialized_bytes: 262144,
        scopes: [
            "context.source.provision",
            "context.measure",
            "context.view.create",
            "context.generate",
            "context.attempt.cancel",
            "context.forget",
            "context.operation.read",
        ]
        .map(str::to_string)
        .into(),
    };
    let host = Host {
        cap: cap.clone(),
        log: Arc::new(Mutex::new(vec![])),
        state: Arc::new(Mutex::new(Default::default())),
        partial,
        wrong_subject,
        lost_source,
        forget_writer: Arc::new(Mutex::new(None)),
        pause_source: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        source_arrived: Arc::new(tokio::sync::Notify::new()),
    };
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", socket.local_addr().unwrap());
    let app = Router::new().fallback(http).with_state(host.clone());
    let task = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
    let cert = contract::Certification {
        model: "fixture".into(),
        principal: l.principal.clone(),
        release: cap.release.clone(),
        runtime: cap.runtime.clone(),
        allocation: cap.allocation_id.clone(),
        endpoint: endpoint.clone(),
        tokenizer_digest: cap.tokenizer_digest.clone(),
        capability: "llm.coding".into(),
        native_tokens: 4000,
        output_reserve: 2000,
        safety_margin: 1000,
        max_input_tokens: 1000,
        max_bytes: 262144,
        expires_at: now() + 60000,
        lease_epoch: 1,
        source_delivery_verified: true,
        cleanup_verified: true,
        base_snapshot_safe: true,
        semantic_verified: true,
        cancellation_verified: true,
    };
    let m = generation::Manifest {
        generation_id: crate::new_id("generation"),
        attempt_id: crate::new_id("attempt"),
        run_id: crate::new_id("run"),
        request_revision: 1,
        input_epoch: l.input_epoch,
        policy_revision: l.policy_revision,
        projection_revision: l.revision,
        purpose: "reasoning-view".into(),
        request_digest: String::new(),
        sources: vec![source.clone()],
        allocation: cap.allocation_id.clone(),
        runtime: cap.runtime.clone(),
        release: cap.release.clone(),
        view_id: None,
        view_digest: None,
        lease_epoch: 1,
        expires_at: now() + 60000,
    };
    let a = managed::Adapter {
        writer: Arc::new(SqliteWriter::from_connection(c)),
        certification: cert,
        client: Client::new(&endpoint, "fixture-secret".into()).unwrap(),
        delivery: Arc::new(managed::UnavailableDelivery),
        product: Some(product::Product {
            can_generate: true,
            capability: cap,
            _lease: None,
        }),
    };
    (a, m, vec![source], host, task)
}
fn request() -> Value {
    json!({"model":"fixture","messages":[{"role":"user","content":"synthetic"}],"max_tokens":2000,"stream":false,"tools":[]})
}
#[tokio::test]
async fn product_recovers_lost_provision_and_uses_v2_attempt_then_seven_phase_cleanup() {
    let (a, m, s, h, t) = fixture(false, false, true).await;
    let (m, _) = inference::infer(&a, m, &request(), &s, Arc::new(RunCancellation::default()))
        .await
        .unwrap();
    assert!(m.view_id.is_some());
    assert!(h
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.starts_with("GET /v1/context-source-operations/")));
    assert_eq!(a.cleanup().await.unwrap(), 1);
    a.writer
        .read_serialized(|c| {
            let count: u64 = c
                .query_row(
                    "SELECT count(*) FROM personal_remote_operations WHERE state!='cleaned'",
                    [],
                    |r| r.get(0),
                )
                .map_err(database_error)?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    t.abort();
}
#[tokio::test]
async fn product_rejects_cross_subject_before_registration() {
    let (a, m, s, h, t) = fixture(false, true, false).await;
    assert!(
        inference::infer(&a, m, &request(), &s, Arc::new(RunCancellation::default()))
            .await
            .is_err()
    );
    assert!(!h
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l == "POST /v1/contexts"));
    t.abort();
}
#[tokio::test]
async fn product_keeps_partial_absence_pending() {
    let (a, m, s, _, t) = fixture(true, false, false).await;
    inference::infer(&a, m, &request(), &s, Arc::new(RunCancellation::default()))
        .await
        .unwrap();
    assert_eq!(a.cleanup().await.unwrap(), 0);
    a.writer
        .read_serialized(|c| {
            assert_eq!(
                c.query_row("SELECT stage FROM personal_cleanup", [], |r| r
                    .get::<_, String>(0))
                    .map_err(database_error)?,
                "pending"
            );
            let diagnostics = commands::snapshot(c)?;
            assert_eq!(
                diagnostics["remoteCleanup"][0]["phases"]["snapshots"],
                "pending"
            );
            assert_eq!(diagnostics["remoteCleanup"][0]["complete"], false);
            Ok(())
        })
        .unwrap();
    t.abort();
}

#[tokio::test]
async fn product_forget_during_delivery_blocks_register_and_tracks_unknown_handle() {
    let (a, m, s, h, t) = fixture(false, false, false).await;
    *h.forget_writer.lock().unwrap() = Some(a.writer.clone());
    assert!(
        inference::infer(&a, m, &request(), &s, Arc::new(RunCancellation::default()))
            .await
            .is_err()
    );
    assert!(!h
        .log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l == "POST /v1/contexts"));
    a.writer.read_serialized(|c|{let leaked:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_remote_operations WHERE request_digest!='' OR json_extract(receipt,'$.sourceDigest') IS NOT NULL)",[],|r|r.get(0)).map_err(database_error)?;assert!(!leaked);Ok(())}).unwrap();
    assert_eq!(a.cleanup().await.unwrap(), 1);
    t.abort();
}
#[tokio::test]
async fn product_off_blocks_new_generation_but_keeps_cleanup() {
    let (mut a, m, s, _, t) = fixture(false, false, false).await;
    inference::infer(
        &a,
        m.clone(),
        &request(),
        &s,
        Arc::new(RunCancellation::default()),
    )
    .await
    .unwrap();
    a.product.as_mut().unwrap().can_generate = false;
    assert!(
        inference::infer(&a, m, &request(), &s, Arc::new(RunCancellation::default()))
            .await
            .is_err()
    );
    assert_eq!(a.cleanup().await.unwrap(), 1);
    t.abort();
}

#[tokio::test]
async fn product_dropped_future_recovers_provision_without_receipt() {
    let (a, m, s, h, t) = fixture(false, false, false).await;
    let id = m.generation_id.clone();
    let a = Arc::new(a);
    h.pause_source
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let running = {
        let a = a.clone();
        tokio::spawn(async move {
            inference::infer(&a, m, &request(), &s, Arc::new(RunCancellation::default())).await
        })
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        h.source_arrived.notified(),
    )
    .await
    .unwrap();
    running.abort();
    let _ = running.await;
    h.pause_source
        .store(false, std::sync::atomic::Ordering::SeqCst);
    a.writer
        .read_serialized(|c| {
            assert!(generation::allow(c, &id).is_err());
            assert_eq!(
                c.query_row(
                    "SELECT status FROM personal_generations WHERE id=?1",
                    [&id],
                    |r| r.get::<_, String>(0)
                )
                .map_err(database_error)?,
                "interrupted"
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(a.cleanup().await.unwrap(), 1);
    t.abort();
}
