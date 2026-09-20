#![cfg(test)]
//! Deterministic G01–G20 fixtures from the implementation guide. Ranker/extractor doubles return
//! fixed values; the same semantics are exercised against real models in the separate ML lane.

#![allow(clippy::too_many_arguments)]

use async_trait::async_trait;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::backends::{BackendOutcome, BackendRequest, FixtureBackend, ToolBackend};
use super::catalog::{self, CatalogEntry, UsagePage};
use super::contracts::*;
use super::extraction::{CorrectionExtractor, FixtureExtractor};
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

/// Backend that parks after recording the call until the test releases it, so the caller future can
/// be aborted while the invocation is genuinely in flight.
struct BarrierBackend {
    entered: Arc<tokio::sync::Semaphore>,
    release: Arc<tokio::sync::Semaphore>,
}

#[async_trait]
impl ToolBackend for BarrierBackend {
    async fn invoke(
        &self,
        _request: BackendRequest,
        _cancellation: &crate::RunCancellation,
    ) -> BackendOutcome {
        self.entered.add_permits(1);
        let _permit = self.release.acquire().await;
        BackendOutcome::succeeded(json!({ "ok": true }))
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
            Arc::new(FixedReranker::new(&[
                ("web-rev1", 2.0),
                ("minutes-rev1", 1.0),
            ])),
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
            operations: spec
                .operations
                .iter()
                .map(|value| value.to_string())
                .collect(),
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
            catalog::register_revision(connection, PRINCIPAL, "llang", &entry, &revision_id, now)
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
                None => {
                    repository::upsert_grant(connection, PRINCIPAL, spec.tool_id, "user", PRINCIPAL)
                }
            }
            .map_err(|error| error.to_string())?;
            repository::bump_epochs(connection, false, true, false)
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("register tool");
        revision_id
    }

    fn register_pair(&self) -> (String, String, String) {
        let mut web = ToolSpec::tool("web", vector(1.0), None);
        web.purpose = "Web search for the latest public information.";
        let mut minutes = ToolSpec::tool("minutes", vector(0.9), None);
        minutes.title = "minutes";
        minutes.purpose = "Internal meeting minutes for decision records.";
        let mut archive = ToolSpec::tool("archive", vector(0.8), None);
        archive.purpose = "Archived document store.";
        (
            self.register(&web),
            self.register(&minutes),
            self.register(&archive),
        )
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

    fn insert_conversation(&self, id: &str) {
        let id = id.to_string();
        let now = crate::now_iso();
        write_tx(&self.writer, move |connection| {
            connection
                .execute(
                    "INSERT OR IGNORE INTO conversations(id, title, task_mode, created_at, updated_at)
                     VALUES (?1, ?1, 'conversation', ?2, ?2)",
                    rusqlite::params![id, now],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("insert conversation");
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
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
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
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(order(&response), vec!["web", "minutes", "archive"]);
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
    assert!(turn
        .apply
        .as_ref()
        .map(|apply| apply.applied)
        .unwrap_or(false));
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(order(&response), vec!["minutes", "web", "archive"]);
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
    let response = service
        .search(&context, "競合の最新情報", 8)
        .await
        .expect("search");
    assert_eq!(order(&response), vec!["web", "minutes", "archive"]);
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
    harness.grant_user("P2", "archive");
    harness.insert_conversation("conversation_b");
    harness.insert_conversation("conversation_p2");
    let other_b = RequestContext::new(PRINCIPAL, "conversation_b")
        .with_run(Some("run-b".into()))
        .with_project(Some("B".to_string()));
    let other_p2 = RequestContext::new("P2", "conversation_p2")
        .with_run(Some("run-p2".into()))
        .with_project(Some("A".to_string()));
    for other in [other_b, other_p2] {
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
        let response = service
            .search(&other, USER_MESSAGE, 8)
            .await
            .expect("search");
        assert_eq!(order(&response), vec!["web", "minutes", "archive"]);
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
    service.set_scenario(&context, Scenario::degraded(USER_MESSAGE));
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(order(&response), vec!["web", "minutes", "archive"]);
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
    assert!(second
        .apply
        .as_ref()
        .map(|apply| apply.duplicate)
        .unwrap_or(false));
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
    service
        .begin_turn(&context, "この案件のフォルダが違う")
        .await;
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
    assert!(turn
        .apply
        .as_ref()
        .map(|apply| apply.ambiguous)
        .unwrap_or(false));
    assert_eq!(harness.count("tool_selection_rules"), 0);
}

#[tokio::test]
async fn g09_successful_invocation_does_not_create_positive_feedback() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
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
async fn p01_caller_abort_does_not_leave_the_invocation_running() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let entered = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let backend: Arc<dyn ToolBackend> = Arc::new(BarrierBackend {
        entered: entered.clone(),
        release: release.clone(),
    });
    let service = Arc::new(harness.service_with_backend(backend, NO_FEEDBACK));
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    let candidate = response.candidates.first().expect("candidate");
    let described = service
        .describe(&context, &candidate.reference, "contract", None)
        .expect("describe");
    let execution_ref = described.execution_ref.expect("execution ref");
    let cancellation = crate::RunCancellation::default();

    let caller = {
        let service = service.clone();
        let context = context.clone();
        let cancellation = cancellation.clone();
        tokio::spawn(async move {
            service
                .invoke(
                    &context,
                    &execution_ref,
                    &json!({ "q": "value" }),
                    &cancellation,
                )
                .await
        })
    };
    let _entry_permit = tokio::time::timeout(std::time::Duration::from_secs(5), entered.acquire())
        .await
        .expect("backend entered within timeout")
        .expect("entry permit");
    // Aborting the caller drops only the response future; the management task must continue.
    caller.abort();
    let _ = caller.await;
    release.add_permits(1);

    let mut status = String::new();
    for _ in 0..400 {
        status = harness
            .writer
            .read_serialized(|connection| {
                connection
                    .query_row(
                        "SELECT technical_status FROM tool_selection_invocations LIMIT 1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(|error| error.to_string())
            })
            .expect("invocation status");
        if status != "running" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(status, "succeeded", "aborted caller left the row unsettled");
}

#[tokio::test]
async fn g10_once_without_task_is_conversation_scoped() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "conversation", "once"));
    service.begin_turn(&context, USER_MESSAGE).await;
    let same = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(order(&same), vec!["minutes", "web", "archive"]);
    let other = RequestContext::new(PRINCIPAL, "conversation_other")
        .with_run(Some("run-other".into()))
        .with_project(Some("A".to_string()));
    harness.insert_conversation("conversation_other");
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
    let other = service
        .search(&other, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(order(&other), vec!["web", "minutes", "archive"]);
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
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(order(&response), vec!["web", "minutes", "archive"]);
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
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
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
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
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
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
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
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
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
        .invoke(
            &context,
            &minutes_execution,
            &json!({"q":"v"}),
            &cancellation,
        )
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
    let service =
        harness.service_with_reranker(Arc::new(super::inference::UnavailableReranker), NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
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
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
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
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    for candidate in &response.candidates {
        assert!(candidate.summary.len() <= super::contracts::SEARCH_CANDIDATE_SUMMARY_MAX_BYTES);
    }
    assert!(response.candidates.len() <= 8);
}

#[tokio::test]
async fn gateway_search_describe_invoke_round_trip() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let cancellation = crate::RunCancellation::default();

    let search = super::gateway::dispatch(
        &service,
        &context,
        super::gateway::TOOL_SEARCH,
        &json!({ "intent": USER_MESSAGE, "limit": 3 }).to_string(),
        &cancellation,
    )
    .await;
    assert_eq!(search.pointer("/ok"), Some(&json!(true)));
    let candidate_ref = search
        .pointer("/data/candidates/0/candidateRef")
        .and_then(Value::as_str)
        .expect("candidate ref")
        .to_string();

    let describe = super::gateway::dispatch(
        &service,
        &context,
        super::gateway::TOOL_DESCRIBE,
        &json!({ "candidateRef": candidate_ref }).to_string(),
        &cancellation,
    )
    .await;
    assert_eq!(describe.pointer("/ok"), Some(&json!(true)));
    let execution_ref = describe
        .pointer("/data/executionRef")
        .and_then(Value::as_str)
        .expect("execution ref")
        .to_string();
    assert!(describe.pointer("/data/body/inputSchema").is_some());

    let invoke = super::gateway::dispatch(
        &service,
        &context,
        super::gateway::TOOL_INVOKE,
        &json!({ "executionRef": execution_ref, "arguments": { "q": "v" } }).to_string(),
        &cancellation,
    )
    .await;
    assert_eq!(invoke.pointer("/ok"), Some(&json!(true)));
    assert_eq!(invoke.pointer("/data/status"), Some(&json!("succeeded")));
}

#[tokio::test]
async fn gateway_rejects_unknown_arguments_without_leaking_details() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let cancellation = crate::RunCancellation::default();
    let response = super::gateway::dispatch(
        &service,
        &context,
        super::gateway::TOOL_SEARCH,
        &json!({ "intent": USER_MESSAGE, "profile": "forged" }).to_string(),
        &cancellation,
    )
    .await;
    assert_eq!(response.pointer("/ok"), Some(&json!(false)));
    assert_eq!(
        response.pointer("/error/code"),
        Some(&json!("invalid-input"))
    );
    assert_eq!(
        response.pointer("/error/message"),
        Some(&json!("The tool selection request is invalid."))
    );
}

// --- Provider-backed extraction (mock HTTP provider) -----------------------------------------

async fn mock_json_provider(body: String) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let endpoint = format!("http://{}/proxy/v1", listener.local_addr().expect("addr"));
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buffer = vec![0_u8; 16 * 1024];
        let _ = socket.read(&mut buffer).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.expect("write");
    });
    (endpoint, task)
}

impl Harness {
    fn configure_mock_provider(&self, endpoint: &str) {
        let endpoint = endpoint.to_string();
        write_tx(&self.writer, move |connection| {
            let mut providers = crate::persistence::load_model_providers(connection)?;
            providers
                .providers
                .push(crate::ModelProviderSettings::OpenAiCompatible(
                    crate::OpenAiCompatibleProviderSettings {
                        request_options: None,
                        id: "mock-extraction-provider".to_string(),
                        enabled: true,
                        label: "Mock extraction".to_string(),
                        location: "local".to_string(),
                        endpoint,
                        model: "fixture".to_string(),
                        authentication: "none".to_string(),
                    },
                ));
            let providers_json =
                serde_json::to_string(&providers).map_err(|error| error.to_string())?;
            connection
                .execute(
                    "UPDATE settings_documents SET value_json = ?1
                      WHERE namespace = 'providers.model' AND key = 'default'",
                    rusqlite::params![providers_json],
                )
                .map_err(|error| error.to_string())?;

            let mut routing = crate::persistence::load_routing_settings(connection)?;
            routing.conversation_respond.source = "provider".to_string();
            routing.conversation_respond.primary_provider_id =
                Some("mock-extraction-provider".to_string());
            let routing_json =
                serde_json::to_string(&routing).map_err(|error| error.to_string())?;
            connection
                .execute(
                    "UPDATE settings_documents SET value_json = ?1
                      WHERE namespace = 'routing.tasks' AND key = 'default'",
                    rusqlite::params![routing_json],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("configure mock provider");
    }
}

#[tokio::test]
async fn provider_extraction_uses_the_configured_conversation_provider() {
    let harness = Harness::new();
    harness.register_pair();
    let extraction = json!({
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
            "rejectedToolId": "web",
            "preferredToolId": "minutes",
            "scope": "project",
            "duration": "persistent",
            "evidence": { "start": 0, "end": 12, "text": "この案件" },
            "condition": {
                "operation": "search",
                "objectType": "decision_record",
                "phase": null,
                "inputKind": null
            }
        }]
    })
    .to_string();
    let chat = json!({
        "model": "fixture",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": extraction },
            "finish_reason": "stop"
        }]
    })
    .to_string();
    let (endpoint, task) = mock_json_provider(chat).await;
    harness.configure_mock_provider(&endpoint);

    let extractor =
        super::provider_extraction::ConversationProviderExtractor::new(harness.writer.clone());
    let request = super::extraction::ExtractionRequest {
        user_message: USER_MESSAGE.to_string(),
        recent_decisions: Vec::new(),
        allowed_decisions: std::collections::HashSet::new(),
        allowed_tools: ["web".to_string(), "minutes".to_string()]
            .into_iter()
            .collect(),
        prompt_tools: vec!["web".to_string(), "minutes".to_string()],
    };
    let parsed = extractor.extract(request).await.expect("extract");
    task.await.expect("mock provider");
    assert_eq!(parsed.accepted.len(), 1);
    assert_eq!(parsed.scenario.object_type, ObjectType::DecisionRecord);
    assert_eq!(
        parsed.accepted[0].preferred_tool_id.as_deref(),
        Some("minutes")
    );
}

#[tokio::test]
async fn provider_extraction_absent_provider_degrades() {
    let harness = Harness::new();
    harness.register_pair();
    let extractor =
        super::provider_extraction::ConversationProviderExtractor::new(harness.writer.clone());
    let request = super::extraction::ExtractionRequest {
        user_message: USER_MESSAGE.to_string(),
        recent_decisions: Vec::new(),
        allowed_decisions: std::collections::HashSet::new(),
        allowed_tools: std::collections::HashSet::new(),
        prompt_tools: Vec::new(),
    };
    assert!(extractor.extract(request).await.is_err());
}

// --- E01: real L-Lang invoke through the selection backend ------------------------------------

impl Harness {
    fn with_writer(writer: Arc<SqliteWriter>) -> Self {
        Self {
            writer,
            backend: Arc::new(FixtureBackend::new()),
        }
    }

    fn service_with_backend(
        &self,
        backend: Arc<dyn super::backends::ToolBackend>,
        extractor_body: &str,
    ) -> ToolSelectionService {
        ToolSelectionService::new(
            self.writer.clone(),
            Arc::new(ConstantEmbedding),
            Arc::new(FixedReranker::new(&[("llang-fixture-rev1", 1.0)])),
            Arc::new(FixtureExtractor::new(extractor_body)),
            backend,
            f64::NEG_INFINITY,
        )
    }

    fn register_llang(
        &self,
        resolved: &crate::generated_capabilities::contracts::ResolvedCapability,
    ) {
        let input_fields: Vec<String> = resolved
            .contract
            .fields
            .iter()
            .map(|field| field.name.clone())
            .collect();
        let mut properties = serde_json::Map::new();
        for field in &input_fields {
            properties.insert(field.clone(), json!({ "type": "boolean" }));
        }
        let input_schema = json!({
            "type": "object",
            "properties": properties,
            "required": input_fields,
            "additionalProperties": false
        });
        let entry = CatalogEntry {
            tool_id: "llang-fixture".to_string(),
            backend_key: "llang-fixture".to_string(),
            title: "llang fixture".to_string(),
            purpose: "Verified L-Lang capability used by the deterministic fixtures.".to_string(),
            operations: vec!["execute".to_string()],
            objects: vec!["code".to_string()],
            suitable: vec!["boolean checks".to_string()],
            unsuitable: vec!["free text".to_string()],
            required_inputs: input_fields.clone(),
            input_schema,
            output_schema: None,
            effect: "pure",
            usage_pages: vec![UsagePage {
                section: "usage",
                page: 0,
                text: "Pass every boolean field.".to_string(),
            }],
            backend_binding: json!({
                "capabilityId": resolved.capability_id,
                "revisionId": resolved.revision_id,
                "packageHash": resolved.package_hash,
                "contractHash": resolved.contract_hash,
                "catalogEpoch": resolved.catalog_epoch,
                "inputFields": input_fields,
            }),
        };
        let revision_id = "llang-fixture-rev1".to_string();
        let now = now_ms();
        write_tx(&self.writer, move |connection| {
            catalog::register_revision(connection, PRINCIPAL, "llang", &entry, &revision_id, now)
                .map_err(|error| error.code.as_str().to_string())?;
            repository::upsert_embedding(connection, &revision_id, MODEL_HASH, &vector(1.0))
                .map_err(|error| error.to_string())?;
            repository::upsert_grant(connection, PRINCIPAL, "llang-fixture", "user", PRINCIPAL)
                .map_err(|error| error.to_string())?;
            repository::bump_epochs(connection, false, true, false)
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("register llang tool");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e01_real_llang_invoke_true_and_false_through_selection() {
    use crate::generated_capabilities::tests::{TestEnv, ACCEPTANCE_A, CANDIDATE_A};
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let resolved = env
        .service
        .resolve_active(&revision.capability_id)
        .expect("resolve active");
    let fields: Vec<String> = resolved
        .contract
        .fields
        .iter()
        .map(|field| field.name.clone())
        .collect();
    let harness = Harness::with_writer(env.writer.clone());
    harness.register_llang(&resolved);

    let backend = Arc::new(super::backends::llang::LlangBackend::new(Some(
        env.service.clone(),
    )));
    let service = harness.service_with_backend(backend, NO_FEEDBACK);
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let response = service
        .search(&context, "verified boolean capability", 8)
        .await
        .expect("search");
    let candidate = response.candidates.first().expect("candidate");
    assert_eq!(candidate.tool_id, "llang-fixture");
    let described = service
        .describe(&context, &candidate.reference, "contract", None)
        .expect("describe");
    let execution_ref = described.execution_ref.expect("execution ref");
    let cancellation = crate::RunCancellation::default();

    for expected in [true, false] {
        // Fixture A is an enabled/suspended truth table: enabled=true, suspended=false is the
        // only true case; every other assignment is false.
        let mut arguments = serde_json::Map::new();
        for field in &fields {
            let value = match field.as_str() {
                "enabled" => expected,
                _ => false,
            };
            arguments.insert(field.clone(), Value::Bool(value));
        }
        let invoked = service
            .invoke(
                &context,
                &execution_ref,
                &Value::Object(arguments),
                &cancellation,
            )
            .await
            .expect("invoke");
        assert_eq!(invoked.status, super::backends::TechnicalStatus::Succeeded);
        let value = invoked
            .result
            .as_ref()
            .and_then(|result| result.get("value"))
            .and_then(Value::as_bool);
        assert_eq!(value, Some(expected));
    }
    assert_eq!(env.call_count(&revision.revision_id), 2);
}

#[tokio::test]
async fn revoke_of_an_unresolvable_tool_keeps_existing_rules() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message.clone()), Some("A"));
    let service = harness.service(&feedback_json("web", "minutes", "project", "persistent"));
    service.begin_turn(&context, USER_MESSAGE).await;
    assert!(harness.count("tool_selection_rules") > 0);

    // A revoke that names a tool the host cannot resolve must not wipe every correction in scope.
    let parsed = ParsedExtraction {
        scenario: Scenario::degraded("撤回"),
        accepted: vec![ExtractedFeedback {
            kind: FeedbackKind::Revoke,
            decision_id: None,
            rejected_tool_id: Some("ghost-tool".into()),
            preferred_tool_id: None,
            scope: ScopeKind::Project,
            duration: Duration::Persistent,
            evidence: Evidence {
                start: 0,
                end: 6,
                text: "この案件".into(),
            },
            condition: FeedbackCondition::default(),
        }],
        rejected: Vec::new(),
    };
    let outcome = service.apply_parsed(&context, &parsed).expect("apply");
    assert!(outcome.ambiguous);
    let active: i64 = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM tool_selection_rules WHERE state = 'active'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("count");
    assert_eq!(active, 2);
}

#[tokio::test]
async fn startup_reconcile_settles_running_invocations() {
    let harness = Harness::new();
    harness.register_pair();
    write_tx(&harness.writer, |connection| {
        connection
            .execute(
                "INSERT INTO tool_selection_invocations(
                   id, decision_id, revision_id, technical_status, satisfaction, started_at)
                 VALUES ('stale-invocation', NULL, 'web-rev1', 'running', 'unknown', 1)",
                [],
            )
            .map_err(|error| error.to_string())?;
        Ok(())
    })
    .expect("insert running invocation");
    let repaired =
        super::service::reconcile_interrupted_invocations(&harness.writer).expect("reconcile");
    assert_eq!(repaired, 1);
    let status: String = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT technical_status FROM tool_selection_invocations
                      WHERE id = 'stale-invocation'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("status");
    assert_eq!(status, "interrupted");
}

#[tokio::test]
async fn explicit_preference_injects_a_tool_retrieval_missed() {
    let harness = Harness::new();
    // 35 noise tools dominate the vector/lexical pools so `minutes` is outside the fused top 30.
    for index in 0..35 {
        let mut noise = ToolSpec::tool(
            Box::leak(format!("noise_{index:02}").into_boxed_str()),
            vector(0.9),
            None,
        );
        noise.purpose = "Unrelated fixture utility.";
        harness.register(&noise);
    }
    let mut minutes = ToolSpec::tool("minutes", vector(0.0), None);
    minutes.purpose = "Internal meeting minutes for decision records.";
    harness.register(&minutes);

    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let body = json!({
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
            "rejectedToolId": null,
            "preferredToolId": "minutes",
            "scope": "project",
            "duration": "persistent",
            "evidence": { "start": 0, "end": 12, "text": "この案件" },
            "condition": {
                "operation": "search",
                "objectType": "decision_record",
                "phase": null,
                "inputKind": null
            }
        }]
    })
    .to_string();
    let service = harness.service_with_reranker(
        Arc::new(FixedReranker::new(&[("minutes-rev1", 5.0)])),
        &body,
    );
    let turn = service.begin_turn(&context, USER_MESSAGE).await;
    assert!(turn
        .apply
        .as_ref()
        .map(|apply| apply.applied)
        .unwrap_or(false));
    let response = service
        .search(&context, "unrelated query text", 8)
        .await
        .expect("search");
    assert_eq!(
        response.candidates.first().map(|c| c.tool_id.as_str()),
        Some("minutes")
    );
}

#[tokio::test]
async fn missing_embedding_index_is_reported_as_degraded_not_no_match() {
    let harness = Harness::new();
    harness.register_pair();
    harness
        .writer
        .write(|connection| {
            connection
                .execute("DELETE FROM tool_selection_embeddings", [])
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("clear embeddings");
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search");
    assert_eq!(response.status, DecisionStatus::Degraded);
    assert!(response.degraded);
}

#[tokio::test]
async fn gateway_rejects_wrong_typed_describe_fields() {
    let harness = Harness::new();
    harness.register_pair();
    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let cancellation = crate::RunCancellation::default();
    let response = super::gateway::dispatch(
        &service,
        &context,
        super::gateway::TOOL_DESCRIBE,
        &json!({ "candidateRef": "whatever", "section": 5 }).to_string(),
        &cancellation,
    )
    .await;
    assert_eq!(response.pointer("/ok"), Some(&json!(false)));
    assert_eq!(
        response.pointer("/error/code"),
        Some(&json!("invalid-input"))
    );
}
