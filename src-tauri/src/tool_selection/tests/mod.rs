//! Deterministic G01–G20 fixtures from the implementation guide. Ranker/extractor doubles return
//! fixed values; the same semantics are exercised against real models in the separate ML lane.

#![allow(clippy::too_many_arguments)]

use async_trait::async_trait;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::backends::FixtureBackend;
use super::catalog::{self, CatalogEntry, UsagePage};
use super::contracts::*;
use super::extraction::FixtureExtractor;
use super::feedback::ParsedExtraction;
use super::inference::{
    EmbedKind, EmbeddingProvider, FixedReranker, InferenceError, RerankProvider,
};
use super::repository::{self, Epochs};
use super::service::ToolSelectionService;
use crate::persistence::SqliteWriter;

const PRINCIPAL: &str = "P1";
const CONVERSATION: &str = "conversation_primary";
const DIM: usize = 8;
const MODEL_HASH: &str = "test-constant";

/// Constant query embedding, so every registered tool with a positive first component is a
/// vector candidate and the reranker decides the order.
struct ConstantEmbedding;

#[async_trait]
impl EmbeddingProvider for ConstantEmbedding {
    fn model_hash(&self) -> &str {
        MODEL_HASH
    }

    fn dimension(&self) -> usize {
        DIM
    }

    async fn embed(
        &self,
        _kind: EmbedKind,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, InferenceError> {
        Ok(texts
            .iter()
            .map(|_| {
                let mut vector = vec![0.0_f32; DIM];
                vector[0] = 1.0;
                vector
            })
            .collect())
    }
}

/// Reranker that bumps the rule epoch once during the first search, proving the epoch re-check
/// causes a second search instead of publishing a stale selection.
struct MutatingReranker {
    inner: FixedReranker,
    writer: Arc<SqliteWriter>,
    calls: AtomicUsize,
    mutated: Mutex<bool>,
}

impl MutatingReranker {
    fn new(writer: Arc<SqliteWriter>, scores: &[(&str, f64)]) -> Self {
        Self {
            inner: FixedReranker::new(scores),
            writer,
            calls: AtomicUsize::new(0),
            mutated: Mutex::new(false),
        }
    }
}

#[async_trait]
impl RerankProvider for MutatingReranker {
    fn model_hash(&self) -> &str {
        self.inner.model_hash()
    }

    async fn rerank(
        &self,
        query: &str,
        documents: &[(String, String)],
    ) -> Result<Vec<(String, f64)>, InferenceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let should_mutate = {
            let mut mutated = self.mutated.lock().expect("mutated");
            if *mutated {
                false
            } else {
                *mutated = true;
                true
            }
        };
        if should_mutate {
            let writer = self.writer.clone();
            let _ = writer.write(|connection| {
                repository::bump_epochs(connection, false, false, true)
                    .map_err(|error| error.to_string())
            });
        }
        self.inner.rerank(query, documents).await
    }
}

struct ToolSpec {
    tool_id: &'static str,
    backend_key: &'static str,
    title: &'static str,
    purpose: &'static str,
    operations: &'static [&'static str],
    objects: &'static [&'static str],
    vector: Vec<f32>,
    project: Option<&'static str>,
    input_schema: Value,
}

impl ToolSpec {
    fn tool(tool_id: &'static str, vector: Vec<f32>, project: Option<&'static str>) -> Self {
        Self {
            tool_id,
            backend_key: tool_id,
            title: tool_id,
            purpose: "Fixture tool for the deterministic selection fixtures.",
            operations: &["search", "read"],
            objects: &["decision_record", "current_information"],
            vector,
            project,
            input_schema: json!({
                "type": "object",
                "properties": { "q": { "type": "string" } },
                "required": ["q"],
                "additionalProperties": false
            }),
        }
    }
}

struct Harness {
    writer: Arc<SqliteWriter>,
    backend: Arc<FixtureBackend>,
}

impl Harness {
    fn new() -> Self {
        let connection = Connection::open_in_memory().expect("in-memory");
        crate::persistence::schema::initialize_database(&connection).expect("schema");
        Self {
            writer: Arc::new(SqliteWriter::from_connection(connection)),
            backend: Arc::new(FixtureBackend::new()),
        }
    }

    fn service(&self, extractor_body: &str) -> ToolSelectionService {
        self.service_with_reranker(
            Arc::new(FixedReranker::new(&[("web-rev1", 2.0), ("minutes-rev1", 1.0)])),
            extractor_body,
        )
    }

    fn service_with_reranker(
        &self,
        reranker: Arc<dyn RerankProvider>,
        extractor_body: &str,
    ) -> ToolSelectionService {
        ToolSelectionService::new(
            self.writer.clone(),
            Arc::new(ConstantEmbedding),
            reranker,
            Arc::new(FixtureExtractor::new(extractor_body)),
            self.backend.clone(),
            f64::NEG_INFINITY,
        )
    }

    fn register(&self, spec: &ToolSpec) -> String {
        let revision_id = format!("{}-rev1", spec.tool_id);
        let entry = CatalogEntry {
            tool_id: spec.tool_id.to_string(),
            backend_key: spec.backend_key.to_string(),
            title: spec.title.to_string(),
            purpose: spec.purpose.to_string(),
            operations: spec.operations.iter().map(|value| value.to_string()).collect(),
            objects: spec.objects.iter().map(|value| value.to_string()).collect(),
            suitable: vec!["decision history lookups".to_string()],
            unsuitable: vec!["unrelated chat".to_string()],
            required_inputs: vec!["q".to_string()],
            input_schema: spec.input_schema.clone(),
            output_schema: None,
            effect: "read",
            usage_pages: vec![UsagePage {
                section: "usage",
                page: 0,
                text: format!("Use {} for the fixture scenario.", spec.tool_id),
            }],
            backend_binding: json!({ "capabilityId": spec.tool_id, "revisionId": revision_id }),
        };
        let now = now_ms();
        write_tx(&self.writer, |connection| {
            catalog::register_revision(
                connection,
                PRINCIPAL,
                "llang",
                &entry,
                &revision_id,
                now,
            )
            .map_err(|error| error.code.as_str().to_string())?;
            repository::upsert_embedding(connection, &revision_id, MODEL_HASH, &spec.vector)
                .map_err(|error| error.to_string())?;
            match spec.project {
                Some(project) => repository::upsert_grant(
                    connection,
                    PRINCIPAL,
                    spec.tool_id,
                    "project",
                    project,
                ),
                None => repository::upsert_grant(
                    connection,
                    PRINCIPAL,
                    spec.tool_id,
                    "user",
                    PRINCIPAL,
                ),
            }
            .map_err(|error| error.to_string())?;
            repository::bump_epochs(connection, false, true, false)
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("register tool");
        revision_id
    }

    fn register_pair(&self) -> (String, String) {
        let mut web = ToolSpec::tool("web", vector(1.0), None);
        web.purpose = "Web search for the latest public information.";
        let mut minutes = ToolSpec::tool("minutes", vector(0.9), None);
        minutes.title = "minutes";
        minutes.purpose = "Internal meeting minutes for decision records.";
        (self.register(&web), self.register(&minutes))
    }

    fn grant_user(&self, principal: &str, tool_id: &str) {
        let principal = principal.to_string();
        let tool_id = tool_id.to_string();
        write_tx(&self.writer, move |connection| {
            repository::upsert_grant(connection, &principal, &tool_id, "user", &principal)
                .map_err(|error| error.to_string())?;
            repository::bump_epochs(connection, false, true, false)
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("grant user");
    }

    fn insert_message(&self) -> String {
        let id = crate::new_id("msg");
        let now = crate::now_iso();
        let message = id.clone();
        write_tx(&self.writer, move |connection| {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                     VALUES (?1, ?2, 'user', 'fixture', ?3)",
                    rusqlite::params![message, CONVERSATION, now],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("insert message");
        id
    }

    fn context(&self, message: Option<String>, project: Option<&str>) -> RequestContext {
        RequestContext::new(PRINCIPAL, CONVERSATION)
            .with_run(Some("run-fixture".to_string()))
            .with_message(message)
            .with_project(project.map(str::to_string))
    }

    fn epochs(&self) -> Epochs {
        self.writer
            .read_serialized(|connection| {
                repository::epochs(connection).map_err(|error| error.to_string())
            })
            .expect("epochs")
    }

    fn count(&self, table: &str) -> i64 {
        self.writer
            .read_serialized(|connection| {
                connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
                    .map_err(|error| error.to_string())
            })
            .expect("count")
    }
}

fn vector(first: f32) -> Vec<f32> {
    let mut value = vec![0.0_f32; DIM];
    value[0] = first;
    value[1] = (1.0 - first * first).max(0.0).sqrt();
    value
}

fn write_tx<T>(
    writer: &SqliteWriter,
    action: impl FnOnce(&Connection) -> Result<T, String>,
) -> Result<T, String> {
    writer.write(|connection| {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(crate::database_error)?;
        let value = action(&transaction)?;
        transaction.commit().map_err(crate::database_error)?;
        Ok(value)
    })
}

fn order(response: &super::service::SearchResponse) -> Vec<String> {
    response
        .candidates
        .iter()
        .map(|candidate| candidate.tool_id.clone())
        .collect()
}

fn feedback_json(rejected: &str, preferred: &str, scope: &str, duration: &str) -> String {
    json!({
        "scenario": {
            "intent": "案件の過去の判断を確認",
            "operation": "search",
            "objectType": "decision_record",
            "phase": "discover",
            "inputKind": "text"
        },
        "feedback": [{
            "kind": "tool_choice",
            "decisionId": null,
            "rejectedToolId": rejected,
            "preferredToolId": preferred,
            "scope": scope,
            "duration": duration,
            "evidence": { "start": 0, "end": 12, "text": "この案件" },
            "condition": {
                "operation": "search",
                "objectType": "decision_record",
                "phase": null,
                "inputKind": null
            }
        }]
    })
    .to_string()
}

const USER_MESSAGE: &str = "この案件の過去の判断はWebでなく議事録で";
const NO_FEEDBACK: &str = r#"{"scenario":{"intent":"案件の過去の判断を確認","operation":"search","objectType":"decision_record","phase":"discover","inputKind":"text"},"feedback":[]}"#;

#[tokio::test]
async fn g01_basic_order_is_reranker_order() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    assert_eq!(order(&response), vec!["web", "minutes"]);
    assert_eq!(response.status, DecisionStatus::Ok);
}

#[tokio::test]
async fn g02_project_correction_applies_to_the_next_search() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    let turn = service.begin_turn(&context, USER_MESSAGE).await;
    assert!(turn.apply.as_ref().map(|apply| apply.applied).unwrap_or(false));
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    assert_eq!(order(&response), vec!["minutes", "web"]);
}

#[tokio::test]
async fn g03_other_object_type_keeps_the_base_order() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    service.set_scenario(
        &context,
        Scenario {
            intent: "競合の最新情報".into(),
            operation: Operation::Search,
            object_type: ObjectType::CurrentInformation,
            phase: Phase::Discover,
            input_kind: InputKind::Text,
        },
    );
    let response = service.search(&context, "競合の最新情報", 8).await.expect("search");
    assert_eq!(order(&response), vec!["web", "minutes"]);
}

#[tokio::test]
async fn g04_other_project_and_principal_are_not_corrected() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    harness.grant_user("P2", "web");
    harness.grant_user("P2", "minutes");
    for other in [
        harness.context(Some(harness.insert_message()), Some("B")),
        RequestContext::new("P2", "conversation_other")
            .with_run(Some("run-other".into()))
            .with_project(Some("A".to_string())),
    ] {
        service.set_scenario(
            &other,
            Scenario {
                intent: USER_MESSAGE.into(),
                operation: Operation::Search,
                object_type: ObjectType::DecisionRecord,
                phase: Phase::Discover,
                input_kind: InputKind::Text,
            },
        );
        let response = service.search(&other, USER_MESSAGE, 8).await.expect("search");
        assert_eq!(order(&response), vec!["web", "minutes"]);
    }
}

#[tokio::test]
async fn g05_unknown_object_does_not_receive_the_correction() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    service.set_scenario(
        &context,
        Scenario::degraded(USER_MESSAGE),
    );
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    assert_eq!(order(&response), vec!["web", "minutes"]);
}

#[tokio::test]
async fn g06_repeated_message_is_idempotent() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    let epoch_after_first = harness.epochs().rule;
    let second = service.begin_turn(&context, USER_MESSAGE).await;
    assert!(second.apply.as_ref().map(|apply| apply.duplicate).unwrap_or(false));
    assert_eq!(harness.epochs().rule, epoch_after_first);
    assert_eq!(harness.count("tool_selection_feedback"), 1);
    assert_eq!(harness.count("tool_selection_rules"), 2); // avoid web + prefer minutes
}

#[tokio::test]
async fn g07_arguments_correction_creates_no_tool_choice_rule() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let body = json!({
        "scenario": {"intent":"フォルダが違う","operation":"unknown","objectType":"unknown","phase":"unknown","inputKind":"unknown"},
        "feedback": [{
            "kind": "arguments",
            "decisionId": null,
            "rejectedToolId": null,
            "preferredToolId": null,
            "scope": "conversation",
            "duration": "once",
            "evidence": {"start":0,"end":9,"text":"この案"},
            "condition": {"operation":null,"objectType":null,"phase":null,"inputKind":null}
        }]
    })
    .to_string();
    let service = harness.service(&body);
    service.begin_turn(&context, "この案件のフォルダが違う").await;
    assert_eq!(harness.count("tool_selection_rules"), 0);
    assert_eq!(harness.count("tool_selection_feedback"), 1);
}

#[tokio::test]
async fn g08_ambiguous_correction_creates_no_rule() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let body = json!({
        "scenario": {"intent":"違う","operation":"unknown","objectType":"unknown","phase":"unknown","inputKind":"unknown"},
        "feedback": [{
            "kind": "ambiguous",
            "decisionId": null,
            "rejectedToolId": null,
            "preferredToolId": null,
            "scope": "conversation",
            "duration": "unspecified",
            "evidence": {"start":0,"end":6,"text":"違う"},
            "condition": {"operation":null,"objectType":null,"phase":null,"inputKind":null}
        }]
    })
    .to_string();
    let service = harness.service(&body);
    let turn = service.begin_turn(&context, "違う").await;
    assert!(turn.apply.as_ref().map(|apply| apply.ambiguous).unwrap_or(false));
    assert_eq!(harness.count("tool_selection_rules"), 0);
}

#[tokio::test]
async fn g09_successful_invocation_does_not_create_positive_feedback() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    let candidate = response.candidates.first().expect("candidate");
    let described = service
        .describe(&context, &candidate.reference, "contract", None)
        .expect("describe");
    let execution_ref = described.execution_ref.expect("execution ref");
    let cancellation = crate::RunCancellation::default();
    let invoked = service
        .invoke(
            &context,
            &execution_ref,
            &json!({ "q": "value" }),
            &cancellation,
        )
        .await
        .expect("invoke");
    assert_ne!(invoked.status, super::backends::TechnicalStatus::Cancelled);
    let satisfaction: String = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT satisfaction FROM tool_selection_invocations LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("satisfaction");
    assert_eq!(satisfaction, "unknown");
}

#[tokio::test]
async fn g10_once_without_task_is_conversation_scoped() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "conversation", "once"));
    service.begin_turn(&context, USER_MESSAGE).await;
    let same = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    assert_eq!(order(&same), vec!["minutes", "web"]);
    let other = RequestContext::new(PRINCIPAL, "conversation_other")
        .with_run(Some("run-other".into()))
        .with_project(Some("A".to_string()));
    service.set_scenario(
        &other,
        Scenario {
            intent: USER_MESSAGE.into(),
            operation: Operation::Search,
            object_type: ObjectType::DecisionRecord,
            phase: Phase::Discover,
            input_kind: InputKind::Text,
        },
    );
    let other = service.search(&other, USER_MESSAGE, 8).await.expect("search");
    assert_eq!(order(&other), vec!["web", "minutes"]);
}

#[tokio::test]
async fn g11_revoke_removes_the_active_rule_and_bumps_epoch() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    let before = harness.epochs().rule;
    let revoke = json!({
        "scenario": {"intent":"撤回","operation":"unknown","objectType":"unknown","phase":"unknown","inputKind":"unknown"},
        "feedback": [{
            "kind": "revoke",
            "decisionId": null,
            "rejectedToolId": "minutes",
            "preferredToolId": null,
            "scope": "project",
            "duration": "persistent",
            "evidence": {"start":0,"end":6,"text":"さっ"},
            "condition": {"operation":null,"objectType":null,"phase":null,"inputKind":null}
        }]
    })
    .to_string();
    let revoke_service = harness.service(&revoke);
    revoke_service
        .begin_turn(&context, "さっきの指定は撤回")
        .await;
    assert!(harness.epochs().rule > before);
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    assert_eq!(order(&response), vec!["web", "minutes"]);
    assert_eq!(harness.count("tool_selection_feedback"), 2);
}

#[tokio::test]
async fn g12_unauthorized_tool_never_appears_even_with_a_preference() {
    let harness = Harness::new();
    let web = ToolSpec::tool("web", vector(1.0), Some("A"));
    harness.register(&web);
    // minutes belongs to project B only, so it is not authorized in project A.
    let minutes = ToolSpec::tool("minutes", vector(0.9), Some("B"));
    harness.register(&minutes);
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    assert!(order(&response).contains(&"web".to_string()));
    assert!(!order(&response).contains(&"minutes".to_string()));
}

#[tokio::test]
async fn g13_rule_commit_during_inference_triggers_one_research() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let reranker = Arc::new(MutatingReranker::new(
        harness.writer.clone(),
        &[("web-rev1", 2.0), ("minutes-rev1", 1.0)],
    ));
    let service = harness.service_with_reranker(reranker.clone(), NO_FEEDBACK);
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    assert_eq!(reranker.calls.load(Ordering::SeqCst), 2);
    assert!(!response.decision_id.is_empty());
}

#[tokio::test]
async fn g14_correction_after_describe_makes_invoke_change_selection() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    let candidate = response.candidates.first().expect("candidate");
    let described = service
        .describe(&context, &candidate.reference, "contract", None)
        .expect("describe");
    let execution_ref = described.execution_ref.expect("execution ref");

    let correction = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    correction.begin_turn(&context, USER_MESSAGE).await;

    let cancellation = crate::RunCancellation::default();
    let error = service
        .invoke(&context, &execution_ref, &json!({"q": "v"}), &cancellation)
        .await
        .expect_err("must change");
    assert_eq!(error.code, ToolSelectionErrorCode::SelectionChanged);
    assert_eq!(harness.backend.call_count(), 0);
}

#[tokio::test]
async fn g15_unknown_decision_id_is_rejected() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let body = json!({
        "scenario": {"intent":"x","operation":"search","objectType":"decision_record","phase":"discover","inputKind":"text"},
        "feedback": [{
            "kind":"tool_choice","decisionId":"not-ours","rejectedToolId":"web","preferredToolId":"minutes",
            "scope":"project","duration":"persistent",
            "evidence":{"start":0,"end":12,"text":"この案件"},
            "condition":{"operation":"search","objectType":"decision_record","phase":null,"inputKind":null}
        }]
    })
    .to_string();
    let service = harness.service(&body);
    service.begin_turn(&context, USER_MESSAGE).await;
    assert_eq!(harness.count("tool_selection_rules"), 0);
    assert_eq!(harness.count("tool_selection_feedback"), 1);
}

#[tokio::test]
async fn g16_invalid_byte_boundary_is_rejected() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let body = json!({
        "scenario": {"intent":"x","operation":"search","objectType":"decision_record","phase":"discover","inputKind":"text"},
        "feedback": [{
            "kind":"tool_choice","decisionId":null,"rejectedToolId":"web","preferredToolId":"minutes",
            "scope":"project","duration":"persistent",
            "evidence":{"start":1,"end":2,"text":"本"},
            "condition":{"operation":"search","objectType":"decision_record","phase":null,"inputKind":null}
        }]
    })
    .to_string();
    let service = harness.service(&body);
    service.begin_turn(&context, USER_MESSAGE).await;
    assert_eq!(harness.count("tool_selection_rules"), 0);
}

#[tokio::test]
async fn g17_storage_failure_rolls_back_and_keeps_the_epoch() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let before = harness.epochs().rule;
    let parsed = ParsedExtraction {
        scenario: Scenario {
            intent: "x".into(),
            operation: Operation::Search,
            object_type: ObjectType::DecisionRecord,
            phase: Phase::Discover,
            input_kind: InputKind::Text,
        },
        accepted: vec![ExtractedFeedback {
            kind: FeedbackKind::ToolChoice,
            decision_id: Some("missing-decision".into()),
            rejected_tool_id: Some("web".into()),
            preferred_tool_id: Some("minutes".into()),
            scope: ScopeKind::Project,
            duration: Duration::Persistent,
            evidence: Evidence {
                start: 0,
                end: 6,
                text: "この案件".into(),
            },
            condition: FeedbackCondition {
                operation: Some(Operation::Search),
                object_type: Some(ObjectType::DecisionRecord),
                phase: None,
                input_kind: None,
            },
        }],
        rejected: Vec::new(),
    };
    let service = harness.service(NO_FEEDBACK);
    let result = service.apply_parsed(&context, &parsed);
    assert!(result.is_err());
    assert_eq!(harness.epochs().rule, before);
    assert_eq!(harness.count("tool_selection_rules"), 0);
}

#[tokio::test]
async fn g18_deleting_the_source_message_removes_derived_rules() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message.clone()), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    assert!(harness.count("tool_selection_rules") > 0);
    write_tx(&harness.writer, {
        let message = message.clone();
        move |connection| {
            connection
                .execute(
                    "DELETE FROM conversation_messages WHERE id = ?1",
                    rusqlite::params![message],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        }
    })
    .expect("delete message");
    assert_eq!(harness.count("tool_selection_rules"), 0);
    assert_eq!(harness.count("tool_selection_feedback"), 0);
}

#[tokio::test]
async fn g19_old_execution_ref_is_stale_after_a_revision_change() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    let minutes = response
        .candidates
        .iter()
        .find(|candidate| candidate.tool_id == "minutes")
        .expect("minutes candidate");
    let described = service
        .describe(&context, &minutes.reference, "contract", None)
        .expect("describe minutes");
    let minutes_execution = described.execution_ref.expect("execution ref");

    // Publish a new revision for the same tool after the reference was issued.
    let entry = CatalogEntry {
        tool_id: "minutes".to_string(),
        backend_key: "minutes".to_string(),
        title: "minutes".to_string(),
        purpose: "Updated minutes tool.".to_string(),
        operations: vec!["search".into()],
        objects: vec!["decision_record".into()],
        suitable: vec![],
        unsuitable: vec![],
        required_inputs: vec!["q".into()],
        input_schema: json!({
            "type": "object",
            "properties": { "q": { "type": "string" } },
            "required": ["q"],
            "additionalProperties": false
        }),
        output_schema: None,
        effect: "read",
        usage_pages: vec![],
        backend_binding: json!({"capabilityId":"minutes","revisionId":"minutes-rev2"}),
    };
    let now = now_ms();
    write_tx(&harness.writer, |connection| {
        catalog::register_revision(connection, PRINCIPAL, "llang", &entry, "minutes-rev2", now)
            .map_err(|error| error.code.as_str().to_string())
    })
    .expect("register revision two");

    let cancellation = crate::RunCancellation::default();
    let error = service
        .invoke(&context, &minutes_execution, &json!({"q":"v"}), &cancellation)
        .await
        .expect_err("stale");
    assert_eq!(error.code, ToolSelectionErrorCode::StaleReference);
    assert_eq!(harness.backend.call_count(), 0);
}

#[tokio::test]
async fn g20_unavailable_reranker_degrades_without_faking_confidence() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service_with_reranker(
        Arc::new(super::inference::UnavailableReranker),
        NO_FEEDBACK,
    );
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    assert_eq!(response.status, DecisionStatus::Degraded);
    assert!(response.degraded);
    assert!(response.notes.iter().any(|note| note.contains("Reranking")));
}

#[tokio::test]
async fn describe_usage_pages_are_paginated_and_bound() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    let candidate = &response.candidates[0];
    let usage = service
        .describe(&context, &candidate.reference, "usage", None)
        .expect("usage");
    assert!(usage.body.get("text").is_some());
    assert!(usage.execution_ref.is_some());
}

#[tokio::test]
async fn search_response_contract_is_bounded() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service.search(&context, USER_MESSAGE, 8).await.expect("search");
    for candidate in &response.candidates {
        assert!(candidate.summary.len() <= super::contracts::SEARCH_CANDIDATE_SUMMARY_MAX_BYTES);
    }
    assert!(response.candidates.len() <= 8);
}
