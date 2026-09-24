use super::*;
pub(super) const SEARCH_CANDIDATE_POOL: usize = 50;
pub(super) const BACKEND_TIMEOUT_MS: u64 = 30_000;
pub(super) const EMBED_BATCH_SIZE: usize = 64;
pub(super) const MAX_SEARCH_ATTEMPTS: usize = 2;
const SCENARIO_CACHE_MAX: usize = 512;
#[derive(Clone, Debug)]
pub struct SearchCandidate {
    pub reference: String,
    pub revision_id: String,
    pub tool_id: String,
    pub source_id: String,
    pub source_label: String,
    pub title: String,
    pub summary: String,
    pub reason: String,
    pub score: f64,
}
#[derive(Clone, Debug)]
pub struct SearchResponse {
    pub decision_id: String,
    pub status: DecisionStatus,
    pub candidates: Vec<SearchCandidate>,
    pub degraded: bool,
    pub notes: Vec<&'static str>,
}
#[derive(Clone, Debug)]
pub struct DescribeResponse {
    pub revision_id: String,
    pub tool_id: String,
    pub source_id: String,
    pub source_label: String,
    pub section: String,
    pub body: Value,
    pub execution_ref: Option<String>,
    pub cursor: Option<String>,
}
#[derive(Clone, Debug)]
pub struct InvokeResponse {
    pub invocation_id: String,
    pub status: TechnicalStatus,
    pub result: Option<Value>,
    pub error_code: Option<&'static str>,
    /// Set when a 16 KiB–1 MiB result was stored for continuation.
    pub result_ref: Option<String>,
    pub byte_count: Option<i64>,
    pub page_count: Option<i64>,
    /// `inline`, `stored` or `unavailable`.
    pub result_availability: Option<&'static str>,
}
#[derive(Clone, Debug)]
pub struct ResultPageResponse {
    pub result_ref: String,
    pub page: i64,
    pub page_count: i64,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct TurnOutcome {
    pub scenario: Scenario,
    pub apply: Option<super::super::feedback::ApplyOutcome>,
    pub degraded: bool,
}
pub(super) struct Snapshot {
    pub(super) epochs: Epochs,
    pub(super) eligible: Vec<EligibleRevision>,
    pub(super) lexical: Vec<String>,
    pub(super) embeddings: Vec<(String, Vec<f32>)>,
    pub(super) rules: Vec<StoredRule>,
}
pub(super) struct RankOutcome {
    pub(super) ordered: Vec<CorrectedCandidate>,
    pub(super) ranks: HashMap<String, (Option<i64>, Option<i64>)>,
    pub(super) raw: HashMap<String, f64>,
    pub(super) status: DecisionStatus,
    pub(super) degraded: bool,
    pub(super) notes: Vec<&'static str>,
    pub(super) adaptive_selection_mode: &'static str,
    pub(super) adaptive_policy_revision: i64,
}
pub struct ToolSelectionService {
    pub(super) writer: Arc<SqliteWriter>,
    pub(super) references: ReferenceStore,
    pub(super) embedding: Arc<dyn EmbeddingProvider>,
    pub(super) reranker: Arc<dyn RerankProvider>,
    pub(super) extractor: Arc<dyn CorrectionExtractor>,
    pub(super) backend: Arc<dyn ToolBackend>,
    pub(super) mcp_manager: Option<Arc<McpManager>>,
    pub(super) no_match_threshold: f64,
    pub(super) scenarios: Mutex<HashMap<String, Scenario>>,
    pub(super) discovery_configured: bool,
}
impl ToolSelectionService {
    pub fn new(
        writer: Arc<SqliteWriter>,
        embedding: Arc<dyn EmbeddingProvider>,
        reranker: Arc<dyn RerankProvider>,
        extractor: Arc<dyn CorrectionExtractor>,
        backend: Arc<dyn ToolBackend>,
        no_match_threshold: f64,
    ) -> Self {
        Self {
            writer,
            references: ReferenceStore::new(),
            embedding,
            reranker,
            extractor,
            backend,
            mcp_manager: None,
            no_match_threshold,
            scenarios: Mutex::new(HashMap::new()),
            discovery_configured: true,
        }
    }
}
impl ToolSelectionService {
    pub fn set_mcp_manager(&mut self, manager: Arc<McpManager>) {
        self.mcp_manager = Some(manager);
    }
}
impl ToolSelectionService {
    pub fn mcp_manager(&self) -> Option<Arc<McpManager>> {
        self.mcp_manager.clone()
    }
}
impl ToolSelectionService {
    pub fn set_discovery_configured(&mut self, configured: bool) {
        self.discovery_configured = configured;
    }
}
impl ToolSelectionService {
    pub fn discovery_configured(&self) -> bool {
        self.discovery_configured
    }
}
impl ToolSelectionService {
    pub fn set_scenario(&self, context: &RequestContext, scenario: Scenario) {
        if let Ok(mut scenarios) = self.scenarios.lock() {
            // Completed runs are never read again; cap the cache so a long session cannot grow it
            // without bound.
            if scenarios.len() >= SCENARIO_CACHE_MAX {
                scenarios.clear();
            }
            scenarios.insert(context.scope_key(), scenario);
        }
    }
}
impl ToolSelectionService {
    pub(super) fn cached_scenario(&self, context: &RequestContext, intent: &str) -> Scenario {
        self.scenarios
            .lock()
            .ok()
            .and_then(|scenarios| scenarios.get(&context.scope_key()).cloned())
            .unwrap_or_else(|| Scenario::degraded(intent))
    }
}
impl ToolSelectionService {
    /// Releases the per-run state an MCP session owns: its cached scenario, its opaque references
    /// and any stored continuation result. Decision, invocation and correction audit rows are
    /// deliberately kept.
    pub fn discard_run_scope(&self, run_id: &str) {
        if let Ok(mut scenarios) = self.scenarios.lock() {
            scenarios.remove(run_id);
        }
        self.references.invalidate_scope(run_id);
        let scope_key = run_id.to_string();
        let _ = self.writer.write(move |connection| {
            super::super::mcp::results::cleanup_scope(connection, &scope_key)
                .map(|_| ())
                .map_err(|error| error.code.as_str().to_string())
        });
    }
}
impl ToolSelectionService {
    pub async fn begin_turn(&self, context: &RequestContext, user_message: &str) -> TurnOutcome {
        let intent = repository::truncate_utf8(user_message, EXTRACT_USER_MESSAGE_MAX_BYTES);
        let (allowed_decisions, allowed_tools, recent, prompt_tools) =
            self.allowed_for_extraction(context);
        let request = ExtractionRequest {
            user_message: intent.to_string(),
            recent_decisions: recent,
            allowed_decisions,
            allowed_tools,
            prompt_tools,
        };
        let parsed = match self.extractor.extract(request).await {
            Ok(parsed) => parsed,
            Err(_) => {
                let scenario = Scenario::degraded(intent);
                self.set_scenario(context, scenario.clone());
                return TurnOutcome {
                    scenario,
                    apply: None,
                    degraded: true,
                };
            }
        };
        let scenario = parsed.scenario.clone();
        self.set_scenario(context, scenario.clone());
        let apply = if parsed.accepted.is_empty() && parsed.rejected.is_empty() {
            None
        } else {
            self.apply_parsed(context, &parsed).ok()
        };
        TurnOutcome {
            scenario,
            apply,
            degraded: false,
        }
    }
}
impl ToolSelectionService {
    pub fn apply_parsed(
        &self,
        context: &RequestContext,
        parsed: &ParsedExtraction,
    ) -> ToolSelectionResult<super::super::feedback::ApplyOutcome> {
        if context.input_message_id.is_none() {
            return Err(ToolSelectionError::invalid());
        }
        let now = now_ms();
        write_transaction(&self.writer, |connection| {
            apply_extraction(connection, context, None, parsed, now)
                .map_err(|error| error.code.as_str().to_string())
        })
        .map_err(|_| ToolSelectionError::storage())
    }
}
impl ToolSelectionService {
    pub(super) fn allowed_for_extraction(
        &self,
        context: &RequestContext,
    ) -> (
        HashSet<String>,
        HashSet<String>,
        Vec<RecentDecision>,
        Vec<String>,
    ) {
        let principal = context.principal_id.clone();
        let conversation = context.conversation_id.clone();
        let project = context.project_id.clone();
        let now = now_ms();
        self.writer
            .read_serialized(|connection| {
                let eligible =
                    repository::eligible_revisions(connection, &principal, project.as_deref(), now)
                        .map_err(|error| error.to_string())?;
                let (eligible, _, _) = crate::artifact_preview::webview_ops::restrict_candidates(
                    &conversation,
                    eligible,
                    Vec::new(),
                    Vec::new(),
                );
                // Count display names so a name published by two sources can be offered to the
                // extractor as a source-qualified alternative instead of an ambiguous bare name.
                let mut name_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for item in &eligible {
                    if let Ok(Some(tool)) =
                        repository::tool_by_id(connection, &item.revision.tool_id)
                    {
                        *name_counts.entry(tool.backend_key).or_insert(0) += 1;
                    }
                }
                let mut tools: HashSet<String> = HashSet::new();
                let mut prompt_candidates: Vec<String> = Vec::new();
                for item in &eligible {
                    tools.insert(item.revision.tool_id.clone());
                    if let Ok(Some(tool)) =
                        repository::tool_by_id(connection, &item.revision.tool_id)
                    {
                        tools.insert(tool.backend_key.clone());
                        let qualified = format!("{}/{}", tool.source_id, tool.backend_key);
                        tools.insert(qualified.clone());
                        if name_counts.get(&tool.backend_key).copied().unwrap_or(0) > 1
                            && !prompt_candidates.contains(&qualified)
                        {
                            prompt_candidates.push(qualified);
                        }
                    }
                }
                let decisions = repository::recent_decisions(
                    connection,
                    &principal,
                    &conversation,
                    EXTRACT_RECENT_DECISIONS,
                )
                .map_err(|error| error.to_string())?;
                let mut allowed_decisions = HashSet::new();
                let mut recent = Vec::new();
                let mut prompt_tools: Vec<String> = Vec::new();
                for (decision_id, scenario, tool_ids) in decisions {
                    allowed_decisions.insert(decision_id.clone());
                    for tool_id in &tool_ids {
                        tools.insert(tool_id.clone());
                        if !prompt_tools.contains(tool_id) {
                            prompt_tools.push(tool_id.clone());
                        }
                    }
                    recent.push(RecentDecision {
                        decision_id,
                        scenario_summary: scenario,
                        tool_ids,
                    });
                }
                prompt_tools.extend(prompt_candidates);
                prompt_tools.sort();
                prompt_tools.truncate(EXTRACT_PROMPT_TOOLS_MAX);
                Ok((allowed_decisions, tools, recent, prompt_tools))
            })
            .unwrap_or_default()
    }
}
impl ToolSelectionService {
    pub(super) async fn snapshot(
        &self,
        context: &RequestContext,
        intent: &str,
    ) -> ToolSelectionResult<Snapshot> {
        let principal = context.principal_id.clone();
        let conversation = context.conversation_id.clone();
        let project = context.project_id.clone();
        let task = context.task_id.clone();
        let model_hash = self.embedding.model_hash().to_string();
        let lexical_ok = retrieval::lexical_eligible(intent);
        let lexical_query = retrieval::fts_match_query(intent);
        let now = now_ms();
        self.writer
            .read_serialized(|connection| {
                let epochs = repository::epochs(connection).map_err(|error| error.to_string())?;
                let eligible =
                    repository::eligible_revisions(connection, &principal, project.as_deref(), now)
                        .map_err(|error| error.to_string())?;
                let lexical = match (lexical_ok, lexical_query.as_deref()) {
                    (true, Some(match_expression)) => repository::lexical_candidates(
                        connection,
                        match_expression,
                        &principal,
                        project.as_deref(),
                        SEARCH_CANDIDATE_POOL,
                        now,
                    )
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .map(|(revision_id, _)| revision_id)
                    .collect(),
                    _ => Vec::new(),
                };
                let embeddings = repository::load_embeddings(
                    connection,
                    &model_hash,
                    &principal,
                    project.as_deref(),
                    now,
                )
                .map_err(|error| error.to_string())?;
                let rules = repository::active_rules(
                    connection,
                    &principal,
                    &conversation,
                    project.as_deref(),
                    task.as_deref(),
                    now,
                )
                .map_err(|error| error.to_string())?;
                let (eligible, lexical, embeddings) =
                    crate::artifact_preview::webview_ops::restrict_candidates(
                        &conversation,
                        eligible,
                        lexical,
                        embeddings,
                    );
                Ok(Snapshot {
                    epochs,
                    eligible,
                    lexical,
                    embeddings,
                    rules,
                })
            })
            .map_err(|_| ToolSelectionError::storage())
    }
}
