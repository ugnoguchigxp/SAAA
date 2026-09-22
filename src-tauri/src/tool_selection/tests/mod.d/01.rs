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
/// A controlled, synthetic evaluation may prove that the Tool Selection wiring obeys an
/// already-approved artifact.  It is deliberately an in-memory fixture: its measurements never
/// enter a user's local history and do not constitute evidence of production improvement.
#[tokio::test]
async fn ai_09_synthetic_approved_artifact_reorders_the_real_tool_search_path() {
    let harness = Harness::new();
    let (web, minutes, archive) = harness.register_pair();
    harness
        .writer
        .write(|connection| {
            let mut settings = crate::persistence::load_role_routing_settings(connection)?;
            settings.adaptive_improvement.enabled = true;
            settings.adaptive_improvement.tool = true;
            connection
                .execute(
                    "UPDATE settings_documents SET value_json=?1 WHERE namespace='routing.roles' AND key='default'",
                    [serde_json::to_string(&settings).map_err(|error| error.to_string())?],
                )
                .map_err(|error| error.to_string())?;

            let candidates = vec![web, minutes.clone(), archive];
            let artifact = crate::adaptive_improvement::create_artifact(
                connection,
                crate::adaptive_improvement::Domain::Tool,
                "A",
                &candidates,
                &serde_json::json!({ minutes.clone(): 0.95, "web-rev1": 0.20, "archive-rev1": 0.10 })
                    .to_string(),
                200,
                1,
            )?;
            assert_eq!(
                crate::adaptive_improvement::apply_evaluation_gate(
                    connection,
                    &artifact,
                    crate::adaptive_improvement::EvaluationGate {
                        examples: 200,
                        recipe_examples: 30,
                        independent_groups: 20,
                        protocol_errors: 0,
                        invalid_sources: 0,
                        scope_leaks: 0,
                        unknown_candidates: 0,
                        success_ci_lower: 0.01,
                        resource_improvement_ci_lower: Some(0.05),
                        other_resource_regression_upper: 0.0,
                    },
                )?,
                "shadow"
            );
            crate::adaptive_improvement::approve_shadow(connection, &artifact)?;
            crate::adaptive_improvement::activate(connection, &artifact, 1, 2)
        })
        .expect("activate synthetic artifact");

    let message = harness.insert_message();
    let context = harness.context(Some(message), Some("A"));
    let service = harness.service(NO_FEEDBACK);
    let response = service
        .search(&context, USER_MESSAGE, 8)
        .await
        .expect("search with approved fixture artifact");

    assert_eq!(order(&response), vec!["minutes", "web", "archive"]);
    let selected_candidate = response.candidates.first().expect("selected Tool");
    let execution_ref = service
        .describe(&context, &selected_candidate.reference, "contract", None)
        .expect("describe selected Tool")
        .execution_ref
        .expect("execution reference");
    let invocation = service
        .invoke(
            &context,
            &execution_ref,
            &json!({"q": "fixture"}),
            &crate::RunCancellation::default(),
        )
        .await
        .expect("invoke selected Tool");
    assert_eq!(
        invocation.status,
        super::backends::TechnicalStatus::Succeeded
    );
    let (selected, mode): (String, String) = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT selected,selection_mode FROM ai_decisions ORDER BY created_at_ms DESC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| error.to_string())
        })
        .expect("adaptive decision");
    assert_eq!((selected, mode), (minutes, "adaptive".into()));
    let technical_success: i64 = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT technical_success FROM ai_outcomes ORDER BY created_at_ms DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("selected Tool outcome");
    assert_eq!(technical_success, 1);
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
