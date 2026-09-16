#![cfg(test)]
//! HTTP orchestration fixtures, not live LARM certification.
use super::{contract::Certification, managed::*, worker::Extractor, *};
use crate::{persistence::sqlite::SqliteWriter, RunCancellation};
use async_trait::async_trait;
use axum::{
    routing::{delete, get, post},
    Json, Router,
};
use saaa_larm_session::contexts::Client;
use saaa_personal_state_core::SourceRef;
use serde_json::{json, Value};
use std::sync::Arc;
struct FixtureDelivery;
#[async_trait]
impl Delivery for FixtureDelivery {
    async fn measure(&self, _: &Value, _: Arc<RunCancellation>) -> Result<u64, String> {
        Ok(10)
    }
    async fn provision(
        &self,
        s: &SourceRef,
        text: &str,
        _: Arc<RunCancellation>,
    ) -> Result<Provision, String> {
        Ok(Provision {
            handle: "fixture-source".into(),
            digest: s.digest.clone(),
            bytes: text.len() as u64,
            tokens: 10,
            tokenizer: "a".repeat(64),
        })
    }
    async fn erase(&self, _: &str) -> Result<(bool, bool), String> {
        Ok((true, true))
    }
}
#[tokio::test]
async fn managed_generation_tracks_registration_view_materialization_and_cleanup() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    crate::initialize_database(&c).unwrap();
    c.execute("INSERT INTO conversation_messages VALUES('fixture-source',?1,'user','この案は保留する','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    let chunk = sources::load(&c, 1, 0, 4096).unwrap();
    let digest = chunk.source.digest.clone();
    let app=Router::new()
        .route("/v1/contexts",post(|Json(mut v):Json<Value>|async move {v["state"]=json!("active");Json(v)}).get(||async{Json(json!({"contexts":[]}))}))
        .route("/v1/contexts/{id}",delete(||async{axum::http::StatusCode::NO_CONTENT}))
        .route("/v1/context-views",post(move |Json(v):Json<Value>| {let digest=digest.clone();async move {
            Json(json!({"schemaVersion":1,"id":crate::new_id("fixture-view"),"operationId":"fixture-operation","allocationId":"a","runtime":"r","release":"release","compatibilityKey":"b".repeat(64),"viewDigest":"c".repeat(64),"canonicalizationVersion":"context-view-v1","baseInputTokens":10,"inputBudgetTokens":100,"tokenCount":20,"orderedItems":v["items"].as_array().unwrap().iter().map(|i|json!({"contextId":i["contextId"],"version":"1","required":true,"utility":1.0,"tokenCount":10,"sourceDigest":digest})).collect::<Vec<_>>(),"omitted":[],"leaseEpoch":1,"state":"ready","createdAt":"2026-01-01T00:00:00Z","expiresAt":"2099-01-01T00:00:00Z"}))
        }}))
        .route("/v1/context-operations/{id}",get(||async{Json(json!({"state":"succeeded"}))}))
        .route("/v1/chat/completions",post(|headers:axum::http::HeaderMap|async move{
            assert!(headers["x-larm-context-view-id"].to_str().unwrap().starts_with("fixture-view"));
            Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"{\"candidates\":[],\"no_change\":true}"}}]}))
        }));
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", socket.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
    let cert = Certification {
        model: "fixture".into(),
        principal: chunk.source.access.principal.clone(),
        release: "release".into(),
        runtime: "r".into(),
        allocation: "a".into(),
        endpoint: endpoint.clone(),
        tokenizer_digest: "a".repeat(64),
        capability: "llm.coding".into(),
        native_tokens: 3000,
        output_reserve: 2000,
        safety_margin: 100,
        max_input_tokens: 100,
        max_bytes: 262144,
        expires_at: now() + 60000,
        lease_epoch: 1,
        source_delivery_verified: true,
        cleanup_verified: true,
        base_snapshot_safe: true,
        semantic_verified: true,
        cancellation_verified: true,
    };
    let writer = Arc::new(SqliteWriter::from_connection(c));
    let adapter = Adapter {
        product: None,
        writer: writer.clone(),
        certification: cert,
        client: Client::new(&endpoint, "fixture".into()).unwrap(),
        delivery: Arc::new(FixtureDelivery),
    };
    let output=adapter.extract(json!({"source":{"ref":chunk.source,"text":chunk.text},"current":[],"instruction":worker::EXTRACTION_INSTRUCTION}),Arc::new(RunCancellation::default())).await.unwrap();
    assert!(output.contains("no_change"));
    writer.read_serialized(|c|{let row:(String,String,String,u64)=c.query_row("SELECT g.status,g.materialization,r.desired,r.pins FROM personal_generations g,personal_registrations r",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();assert_eq!(row,("succeeded".into(),"succeeded".into(),"deleted".into(),0));Ok(())}).unwrap();
    assert_eq!(adapter.cleanup().await.unwrap(), 1);
    writer
        .read_serialized(|c| {
            assert_eq!(
                c.query_row("SELECT stage FROM personal_cleanup", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                "complete"
            );
            Ok(())
        })
        .unwrap();
    writer.write(|c|{c.execute("INSERT INTO conversation_messages VALUES('second-source',?1,'user','この案は保留する','2')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();Ok(())}).unwrap();
    let (ledger, required) = writer
        .read_serialized(|c| {
            Ok((
                store::load(c)?,
                vec![
                    sources::load(c, 1, 0, 4096)?.source,
                    sources::load(c, 2, 0, 4096)?.source,
                ],
            ))
        })
        .unwrap();
    let m = generation::Manifest {
        generation_id: crate::new_id("generation"),
        attempt_id: crate::new_id("attempt"),
        run_id: "multi".into(),
        request_revision: 1,
        input_epoch: ledger.input_epoch,
        policy_revision: ledger.policy_revision,
        projection_revision: ledger.revision,
        purpose: "reasoning-view".into(),
        request_digest: String::new(),
        sources: required.clone(),
        allocation: "a".into(),
        runtime: "r".into(),
        release: "release".into(),
        view_id: None,
        view_digest: None,
        lease_epoch: 1,
        expires_at: now() + 60000,
    };
    let request = json!({"model":"fixture","messages":[],"max_tokens":2000,"stream":false});
    let (completed, _) = inference::infer(
        &adapter,
        m.clone(),
        &request,
        &required,
        Arc::new(RunCancellation::default()),
    )
    .await
    .unwrap();
    assert!(completed.view_id.is_some());
    assert_eq!(
        completed.request_digest,
        generation::request_digest(&request).unwrap()
    );
    assert_eq!(adapter.cleanup().await.unwrap(), 2);
    // Reusing the same attempt cannot dispatch a second inference.
    assert!(inference::infer(
        &adapter,
        m,
        &request,
        &required,
        Arc::new(RunCancellation::default())
    )
    .await
    .is_err());
    assert_eq!(adapter.cleanup().await.unwrap(), 0);
    server.abort();
}
