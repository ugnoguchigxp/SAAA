//! Orchestration for the three entry points plus correction ingestion. Reads snapshot the ledger
//! and release the writer lock before any inference; writes re-check epochs inside one immediate
//! transaction so a rule or ACL change cannot be published behind an older selection.

#![allow(private_interfaces)]

use base64::Engine;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::backends::mcp::McpBinding;
use super::backends::router::BackendRouter;
use super::backends::{BackendRequest, TechnicalStatus, ToolBackend};
use super::catalog::{self, CatalogEntry};
use super::contracts::*;
use super::extraction::{CorrectionExtractor, ExtractionRequest, RecentDecision};
use super::feedback::{apply_extraction, ParsedExtraction};
use super::inference::{EmbedKind, EmbeddingProvider, RerankProvider};
use super::mcp::manager::McpManager;
use super::references::{ReferenceEntry, ReferenceKind, ReferenceStore};
use super::repository::{self, EligibleRevision, Epochs};
use super::{ranking, retrieval, rules};
use crate::persistence::{load_role_routing_settings, SqliteWriter};
use crate::RunCancellation;

const SEARCH_CANDIDATE_POOL: usize = 50;
const BACKEND_TIMEOUT_MS: u64 = 30_000;
const EMBED_BATCH_SIZE: usize = 64;
const MAX_SEARCH_ATTEMPTS: usize = 2;
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
    pub apply: Option<super::feedback::ApplyOutcome>,
    pub degraded: bool,
}

struct Snapshot {
    epochs: Epochs,
    eligible: Vec<EligibleRevision>,
    lexical: Vec<String>,
    embeddings: Vec<(String, Vec<f32>)>,
    rules: Vec<StoredRule>,
}

struct RankOutcome {
    ordered: Vec<CorrectedCandidate>,
    ranks: HashMap<String, (Option<i64>, Option<i64>)>,
    raw: HashMap<String, f64>,
    status: DecisionStatus,
    degraded: bool,
    notes: Vec<&'static str>,
    adaptive_selection_mode: &'static str,
    adaptive_policy_revision: i64,
}

pub struct ToolSelectionService {
    writer: Arc<SqliteWriter>,
    references: ReferenceStore,
    embedding: Arc<dyn EmbeddingProvider>,
    reranker: Arc<dyn RerankProvider>,
    extractor: Arc<dyn CorrectionExtractor>,
    backend: Arc<dyn ToolBackend>,
    mcp_manager: Option<Arc<McpManager>>,
    no_match_threshold: f64,
    scenarios: Mutex<HashMap<String, Scenario>>,
    discovery_configured: bool,
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

    pub fn set_mcp_manager(&mut self, manager: Arc<McpManager>) {
        self.mcp_manager = Some(manager);
    }

    pub fn mcp_manager(&self) -> Option<Arc<McpManager>> {
        self.mcp_manager.clone()
    }

    pub fn set_discovery_configured(&mut self, configured: bool) {
        self.discovery_configured = configured;
    }

    pub fn discovery_configured(&self) -> bool {
        self.discovery_configured
    }

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

    fn cached_scenario(&self, context: &RequestContext, intent: &str) -> Scenario {
        self.scenarios
            .lock()
            .ok()
            .and_then(|scenarios| scenarios.get(&context.scope_key()).cloned())
            .unwrap_or_else(|| Scenario::degraded(intent))
    }

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
            super::mcp::results::cleanup_scope(connection, &scope_key)
                .map(|_| ())
                .map_err(|error| error.code.as_str().to_string())
        });
    }

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

    pub fn apply_parsed(
        &self,
        context: &RequestContext,
        parsed: &ParsedExtraction,
    ) -> ToolSelectionResult<super::feedback::ApplyOutcome> {
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

    fn allowed_for_extraction(
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

    async fn snapshot(
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

    async fn rank(
        &self,
        context: &RequestContext,
        scenario: &Scenario,
        intent: &str,
        snapshot: &Snapshot,
    ) -> ToolSelectionResult<RankOutcome> {
        let mut notes = Vec::new();
        let revision_by_id: HashMap<&str, &EligibleRevision> = snapshot
            .eligible
            .iter()
            .map(|item| (item.revision.id.as_str(), item))
            .collect();
        let tool_revision: HashMap<&str, &str> = snapshot
            .eligible
            .iter()
            .map(|item| (item.revision.tool_id.as_str(), item.revision.id.as_str()))
            .collect();

        let query = retrieval::query_text(intent);
        let mut vector_ids: Vec<String> = Vec::new();
        // A missing index is treated as an unavailable embedding branch, not as "no match".
        let mut vector_degraded = self.embedding.dimension() == 0
            || (snapshot.embeddings.is_empty() && !snapshot.eligible.is_empty());
        if !vector_degraded {
            match self
                .embedding
                .embed(EmbedKind::Query, std::slice::from_ref(&query))
                .await
            {
                Ok(vectors) => {
                    if vectors.first().map(Vec::len) != Some(self.embedding.dimension()) {
                        return Err(ToolSelectionError::new(
                            ToolSelectionErrorCode::Integrity,
                            "The embedding model returned an unexpected dimension.",
                        ));
                    }
                    let scored = retrieval::embedding_candidates(&vectors[0], &snapshot.embeddings);
                    vector_ids = scored
                        .into_iter()
                        .take(SEARCH_CANDIDATE_POOL)
                        .map(|(revision_id, _)| revision_id)
                        .collect();
                }
                Err(_) => {
                    vector_degraded = true;
                    notes.push("Embedding search is unavailable; lexical search was used.");
                }
            }
        } else {
            notes.push("Embedding search is unavailable; lexical search was used.");
        }

        let mut fused = retrieval::fuse_candidates(&snapshot.lexical, &vector_ids, RERANK_TOP); // Inject explicitly preferred tools that retrieval missed, authorized only. Filter by
                                                                                                // scope/condition before dedupe so a non-matching narrow rule cannot mask a matching
                                                                                                // broad one.
        let matching_prefer: Vec<&StoredRule> = snapshot
            .rules
            .iter()
            .filter(|rule| {
                rule.action == RuleAction::Prefer
                    && rules::scope_matches(rule, context)
                    && rules::condition_matches(rule, scenario)
            })
            .collect();
        let mut injected = 0;
        for rule in rules::dedupe_by_condition(&matching_prefer) {
            if injected >= RERANK_PREFERRED_MAX {
                break;
            }
            let Some(tool_id) = rule.target_tool_id.as_deref() else {
                continue;
            };
            if let Some(revision_id) = tool_revision.get(tool_id) {
                if !fused.iter().any(|item| item.revision_id == *revision_id) {
                    fused.push(ranking::FusedCandidate {
                        revision_id: (*revision_id).to_string(),
                        lex_rank: None,
                        vec_rank: None,
                        score: 0.0,
                    });
                    injected += 1;
                }
            }
        }
        fused.retain(|item| revision_by_id.contains_key(item.revision_id.as_str()));

        let documents: Vec<(String, String)> = fused
            .iter()
            .filter_map(|item| {
                revision_by_id
                    .get(item.revision_id.as_str())
                    .map(|revision| {
                        (
                            revision.revision.id.clone(),
                            retrieval::document_text(&revision.revision.search_text),
                        )
                    })
            })
            .collect();

        let mut raw: HashMap<String, f64> = HashMap::new();
        let mut ordered_ids: Vec<String>;
        let mut rerank_degraded = false;
        match self.reranker.rerank(&query, &documents).await {
            Ok(scores) if scores.len() == documents.len() => {
                let mut scored = scores;
                scored.sort_by(|left, right| {
                    right
                        .1
                        .partial_cmp(&left.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| left.0.cmp(&right.0))
                });
                ordered_ids = Vec::with_capacity(scored.len());
                for (revision_id, score) in scored {
                    let Some(score) = ranking::finite_score(score) else {
                        return Err(ToolSelectionError::new(
                            ToolSelectionErrorCode::Integrity,
                            "The reranker returned a non-finite score.",
                        ));
                    };
                    raw.insert(revision_id.clone(), score);
                    ordered_ids.push(revision_id);
                }
            }
            Ok(_) => {
                rerank_degraded = true;
                notes.push("Reranking returned an unexpected shape; hybrid order was used.");
                ordered_ids = fused.iter().map(|item| item.revision_id.clone()).collect();
            }
            Err(_) => {
                rerank_degraded = true;
                notes.push("Reranking is unavailable; hybrid order was used.");
                ordered_ids = fused.iter().map(|item| item.revision_id.clone()).collect();
            }
        }
        // Candidates injected for a preference but missing from the reranker list are appended.
        for item in &fused {
            if !ordered_ids.contains(&item.revision_id) {
                ordered_ids.push(item.revision_id.clone());
            }
        }

        let total = ordered_ids.len();
        let base_candidates: Vec<rules::BaseCandidate> = ordered_ids
            .iter()
            .enumerate()
            .filter_map(|(index, revision_id)| {
                revision_by_id
                    .get(revision_id.as_str())
                    .map(|revision| rules::BaseCandidate {
                        revision_id: revision_id.clone(),
                        tool_id: revision.revision.tool_id.clone(),
                        base_score: ranking::base_from_rank(index + 1, total),
                    })
            })
            .collect();
        let correction = rules::apply_rules(&base_candidates, &snapshot.rules, scenario, context);
        if correction.ambiguous {
            notes.push("A conflicting preference was not applied for this search.");
        }

        let mut ranks: HashMap<String, (Option<i64>, Option<i64>)> = fused
            .iter()
            .map(|item| {
                (
                    item.revision_id.clone(),
                    (
                        item.lex_rank.map(|rank| rank as i64),
                        item.vec_rank.map(|rank| rank as i64),
                    ),
                )
            })
            .collect();
        for revision_id in &ordered_ids {
            ranks.entry(revision_id.clone()).or_insert((None, None));
        }

        let top_raw = raw.values().copied().fold(f64::NEG_INFINITY, f64::max);
        let status = if vector_degraded || rerank_degraded {
            DecisionStatus::Degraded
        } else if raw.is_empty() || top_raw < self.no_match_threshold {
            DecisionStatus::NoMatch
        } else {
            DecisionStatus::Ok
        };
        // The adaptive layer runs only after ACL, retrieval, and explicit corrections have
        // produced this finite candidate set. It can move an already-authorized revision to the
        // front; it cannot add a tool or bypass invocation validation.
        let mut adaptive_ordered = correction.ordered;
        let mut adaptive_selection_mode = "rules";
        let mut adaptive_policy_revision = 0;
        if let Some(rules_top) = adaptive_ordered
            .first()
            .map(|candidate| candidate.revision_id.clone())
        {
            let candidate_ids = adaptive_ordered
                .iter()
                .map(|candidate| candidate.revision_id.clone())
                .collect::<Vec<_>>();
            let scope = context.project_id.as_deref().unwrap_or("global");
            if let Ok(Some((selected, mode, revision))) =
                self.writer.read_serialized(|connection| {
                    let settings = load_role_routing_settings(connection)?;
                    if settings.adaptive_improvement.enabled && settings.adaptive_improvement.tool {
                        crate::adaptive_improvement::choose(
                            connection,
                            crate::adaptive_improvement::Domain::Tool,
                            scope,
                            &candidate_ids,
                            &rules_top,
                            now_ms(),
                        )
                        .map(Some)
                    } else {
                        Ok(None)
                    }
                })
            {
                if mode != "rules" {
                    if let Some(index) = adaptive_ordered
                        .iter()
                        .position(|candidate| candidate.revision_id == selected)
                    {
                        adaptive_ordered.swap(0, index);
                        notes.push("An approved adaptive policy reordered eligible tools.");
                        adaptive_selection_mode = mode;
                        adaptive_policy_revision = revision;
                    }
                }
            }
        }
        Ok(RankOutcome {
            ordered: adaptive_ordered,
            ranks,
            raw,
            status,
            degraded: vector_degraded || rerank_degraded,
            notes,
            adaptive_selection_mode,
            adaptive_policy_revision,
        })
    }

    pub async fn search(
        &self,
        context: &RequestContext,
        intent: &str,
        limit: usize,
    ) -> ToolSelectionResult<SearchResponse> {
        let intent_trimmed = intent.trim();
        let scenario = self.cached_scenario(context, intent_trimmed);
        self.search_with_scenario(context, intent_trimmed, limit, &scenario)
            .await
    }

    /// Extracts only the scenario for one host call. Correction candidates are deliberately
    /// discarded and the session scenario cache is not touched, so an external MCP intent can
    /// never be persisted as a user correction or bleed into another search.
    pub async fn extract_scenario_only(
        &self,
        context: &RequestContext,
        user_message: &str,
    ) -> Scenario {
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
        match self.extractor.extract(request).await {
            Ok(parsed) => parsed.scenario,
            Err(_) => Scenario::degraded(intent),
        }
    }

    /// Search with a request-local scenario. The scenario is never written to the shared session
    /// cache, so concurrent searches in one session cannot observe each other's intent.
    pub async fn search_with_scenario(
        &self,
        context: &RequestContext,
        intent: &str,
        limit: usize,
        scenario: &Scenario,
    ) -> ToolSelectionResult<SearchResponse> {
        let intent = intent.trim();
        if intent.is_empty() || intent.len() > SEARCH_INTENT_MAX_BYTES {
            return Err(ToolSelectionError::invalid());
        }
        if !(1..=SEARCH_LIMIT_MAX).contains(&limit) {
            return Err(ToolSelectionError::invalid());
        }
        let mut outcome = None;
        for _ in 0..MAX_SEARCH_ATTEMPTS {
            let snapshot = self.snapshot(context, intent).await?;
            let ranking = self.rank(context, scenario, intent, &snapshot).await?;
            let decision_id = crate::new_id("tsdecision");
            match self.persist(context, scenario, &decision_id, &snapshot, &ranking) {
                Ok(()) => {
                    outcome = Some((snapshot.epochs, decision_id, ranking));
                    break;
                }
                Err(PersistError::Changed) => continue,
                Err(PersistError::Storage) => return Err(ToolSelectionError::storage()),
            }
        }
        let Some((epochs, decision_id, ranking)) = outcome else {
            return Err(ToolSelectionError::changed());
        };
        let mut candidates = Vec::new();
        for candidate in ranking.ordered.iter().take(limit) {
            let Some(revision) = self.read_revision(&candidate.revision_id)? else {
                continue;
            };
            let (title, summary) = title_and_summary(&revision.search_text);
            let reason = if candidate.rule_ids.is_empty() {
                "Hybrid lexical and semantic match.".to_string()
            } else {
                "Adjusted by a saved correction for this condition.".to_string()
            };
            let entry = ReferenceEntry {
                kind: ReferenceKind::Candidate,
                decision_id: decision_id.clone(),
                revision_id: candidate.revision_id.clone(),
                tool_id: candidate.tool_id.clone(),
                schema_hash: revision.schema_hash.clone(),
                principal_id: context.principal_id.clone(),
                conversation_id: context.conversation_id.clone(),
                run_id: context.run_id.clone(),
                project_id: context.project_id.clone(),
                task_id: context.task_id.clone(),
                catalog_epoch: epochs.catalog,
                acl_epoch: epochs.acl,
                rule_epoch: epochs.rule,
                input_schema: revision.input_schema.clone(),
                created_at_ms: now_ms(),
            };
            let reference = self.references.issue(entry, now_ms())?;
            let (source_id, source_label) =
                super::mcp::service_support::source_display(&self.writer, &candidate.tool_id);
            candidates.push(SearchCandidate {
                reference,
                revision_id: candidate.revision_id.clone(),
                tool_id: candidate.tool_id.clone(),
                source_id,
                source_label,
                title,
                summary,
                reason,
                score: candidate.final_score,
            });
        }
        Ok(SearchResponse {
            decision_id,
            status: ranking.status,
            degraded: ranking.degraded,
            candidates,
            notes: ranking.notes,
        })
    }

    fn persist(
        &self,
        context: &RequestContext,
        scenario: &Scenario,
        decision_id: &str,
        snapshot: &Snapshot,
        ranking: &RankOutcome,
    ) -> Result<(), PersistError> {
        let now = now_ms();
        let decision = DecisionRecord {
            id: decision_id.to_string(),
            principal_id: context.principal_id.clone(),
            conversation_id: context.conversation_id.clone(),
            run_id: context.run_id.clone(),
            message_id: context.input_message_id.clone(),
            scenario: scenario.clone(),
            catalog_epoch: snapshot.epochs.catalog,
            acl_epoch: snapshot.epochs.acl,
            rule_epoch: snapshot.epochs.rule,
            model_hash: Some(self.embedding.model_hash().to_string()),
            status: ranking.status,
            created_at: now,
            candidates: ranking
                .ordered
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    let (lex_rank, vec_rank) = ranking
                        .ranks
                        .get(&candidate.revision_id)
                        .copied()
                        .unwrap_or((None, None));
                    CandidateRecord {
                        revision_id: candidate.revision_id.clone(),
                        tool_id: candidate.tool_id.clone(),
                        lex_rank,
                        vec_rank,
                        raw_score: ranking.raw.get(&candidate.revision_id).copied(),
                        base_score: candidate.base_score,
                        final_score: candidate.final_score,
                        rule_ids: candidate.rule_ids.clone(),
                        final_rank: index as i64,
                    }
                })
                .collect(),
        };
        let expected = snapshot.epochs;
        write_transaction(&self.writer, |connection| {
            let current = repository::epochs(connection).map_err(|_| "storage".to_string())?;
            if current != expected {
                return Err(CHANGED.to_string());
            }
            repository::insert_decision(connection, &decision)
                .map_err(|_| "storage".to_string())?;
            let eligible = ranking
                .ordered
                .iter()
                .map(|candidate| candidate.revision_id.clone())
                .collect::<Vec<_>>();
            if let Some(selected) = eligible.first().cloned() {
                let scope = context.project_id.as_deref().unwrap_or("global");
                crate::adaptive_improvement::record_decision(
                    connection,
                    &crate::adaptive_improvement::DecisionObservation {
                        id: format!("ai-tool-{decision_id}"),
                        domain: crate::adaptive_improvement::Domain::Tool,
                        scope_key: scope.to_string(),
                        event_seq: 0,
                        policy_revision: ranking.adaptive_policy_revision,
                        candidate_fingerprint: crate::adaptive_improvement::fingerprint_for(
                            &eligible,
                        ),
                        eligible_candidates: eligible,
                        selected,
                        selection_mode: ranking.adaptive_selection_mode.to_string(),
                        source_refs_json:
                            serde_json::json!({"toolSelectionDecisionId":decision_id}).to_string(),
                    },
                    now,
                )?;
            }
            Ok(())
        })
        .map_err(|error| {
            if error == CHANGED {
                PersistError::Changed
            } else {
                PersistError::Storage
            }
        })
    }

    fn read_revision(
        &self,
        revision_id: &str,
    ) -> ToolSelectionResult<Option<repository::RevisionRow>> {
        self.writer
            .read_serialized(|connection| {
                repository::revision_by_id(connection, revision_id)
                    .map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())
    }

    pub fn describe(
        &self,
        context: &RequestContext,
        candidate_ref: &str,
        section: &str,
        cursor: Option<&str>,
    ) -> ToolSelectionResult<DescribeResponse> {
        if !matches!(
            section,
            "contract" | "usage" | "examples" | "troubleshooting"
        ) {
            return Err(ToolSelectionError::invalid());
        }
        let reference = self
            .references
            .resolve(candidate_ref, ReferenceKind::Candidate, now_ms())
            .ok_or_else(ToolSelectionError::not_found)?;
        if reference.principal_id != context.principal_id
            || reference.scope_key() != context.scope_key()
        {
            return Err(ToolSelectionError::unauthorized());
        }
        let (tool, revision) = self.current_revision(&reference)?;
        let (source_id, source_label) =
            super::mcp::service_support::source_display(&self.writer, &revision.tool_id);
        if section == "contract" {
            let body = json!({
                "revisionId": revision.id,
                "title": tool.backend_key,
                "inputSchema": revision.input_schema,
                "outputSchema": revision.output_schema,
            });
            if serde_json::to_vec(&body)
                .map(|bytes| bytes.len())
                .unwrap_or(usize::MAX)
                > DESCRIBE_RESPONSE_MAX_BYTES
            {
                // A contract that cannot be described is not runnable, so no execution ref is
                // issued.
                return Err(ToolSelectionError::new(
                    ToolSelectionErrorCode::Unavailable,
                    "The tool contract is too large to describe.",
                ));
            }
            let execution_ref = self.issue_execution_ref(context, &reference, &revision)?;
            return Ok(DescribeResponse {
                revision_id: revision.id,
                tool_id: revision.tool_id,
                source_id,
                source_label,
                section: section.to_string(),
                body,
                execution_ref: Some(execution_ref),
                cursor: None,
            });
        }
        let execution_ref = self.issue_execution_ref(context, &reference, &revision)?;
        let page = decode_cursor(cursor, &reference.revision_id, section)?;
        let text = self
            .writer
            .read_serialized(|connection| {
                repository::usage_page(connection, &reference.revision_id, section, page)
                    .map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())?
            .ok_or_else(ToolSelectionError::not_found)?;
        let has_next = self
            .writer
            .read_serialized(|connection| {
                repository::usage_page(connection, &reference.revision_id, section, page + 1)
                    .map(|value| value.is_some())
                    .map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())?;
        let body = json!({ "section": section, "page": page, "text": text });
        Ok(DescribeResponse {
            revision_id: revision.id,
            tool_id: revision.tool_id,
            source_id,
            source_label,
            section: section.to_string(),
            body,
            execution_ref: Some(execution_ref),
            cursor: has_next.then(|| encode_cursor(&reference.revision_id, section, page + 1)),
        })
    }

    fn issue_execution_ref(
        &self,
        context: &RequestContext,
        candidate: &ReferenceEntry,
        revision: &repository::RevisionRow,
    ) -> ToolSelectionResult<String> {
        let tool = self
            .writer
            .read_serialized(|connection| {
                repository::tool_by_id(connection, &revision.tool_id)
                    .map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())?
            .ok_or_else(ToolSelectionError::not_found)?;
        let entry = ReferenceEntry {
            kind: ReferenceKind::Execution,
            decision_id: candidate.decision_id.clone(),
            revision_id: revision.id.clone(),
            tool_id: tool.id.clone(),
            schema_hash: revision.schema_hash.clone(),
            principal_id: context.principal_id.clone(),
            conversation_id: context.conversation_id.clone(),
            run_id: context.run_id.clone(),
            project_id: context.project_id.clone(),
            task_id: context.task_id.clone(),
            catalog_epoch: candidate.catalog_epoch,
            acl_epoch: candidate.acl_epoch,
            rule_epoch: candidate.rule_epoch,
            input_schema: revision.input_schema.clone(),
            created_at_ms: now_ms(),
        };
        self.references.issue(entry, now_ms())
    }

    fn current_revision(
        &self,
        reference: &ReferenceEntry,
    ) -> ToolSelectionResult<(repository::ToolRow, repository::RevisionRow)> {
        let principal = reference.principal_id.clone();
        let project = reference.project_id.clone();
        let tool_id = reference.tool_id.clone();
        let revision_id = reference.revision_id.clone();
        let now = now_ms();
        let result = self
            .writer
            .read_serialized(move |connection| {
                let tool = repository::tool_by_id(connection, &tool_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "not-found".to_string())?;
                if !tool.enabled
                    || tool.current_revision_id.as_deref() != Some(revision_id.as_str())
                {
                    return Err("stale".to_string());
                }
                // A residual reference must be refused when the source is disabled or stale,
                // even if no catalog epoch has moved since the reference was issued.
                if !super::source_lookup::source_eligible(connection, &tool.source_id, now)
                    .map_err(|error| error.to_string())?
                {
                    return Err("stale".to_string());
                }
                let authorized =
                    repository::grant_exists(connection, &principal, &tool_id, project.as_deref())
                        .map_err(|error| error.to_string())?;
                if !authorized {
                    return Err("unauthorized".to_string());
                }
                let revision = repository::revision_by_id(connection, &revision_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "not-found".to_string())?;
                Ok((tool, revision))
            })
            .map_err(|error| match error.as_str() {
                "stale" => ToolSelectionError::stale(),
                "unauthorized" => ToolSelectionError::unauthorized(),
                "not-found" => ToolSelectionError::not_found(),
                _ => ToolSelectionError::storage(),
            })?;
        Ok(result)
    }

    /// Resolves the trusted effect for an execution reference without opening an invocation.
    /// Role routing uses this immediately before its own atomic reservation; the later invoke
    /// repeats all reference/epoch checks before the backend owner is started.
    pub(crate) fn execution_effect(
        &self,
        context: &RequestContext,
        execution_ref: &str,
    ) -> ToolSelectionResult<String> {
        let reference = self
            .references
            .resolve(execution_ref, ReferenceKind::Execution, now_ms())
            .ok_or_else(ToolSelectionError::not_found)?;
        if reference.principal_id != context.principal_id
            || reference.scope_key() != context.scope_key()
        {
            return Err(ToolSelectionError::unauthorized());
        }
        let (_, revision) = self.current_revision(&reference)?;
        let current_epochs = self.read_epochs()?;
        if current_epochs.rule != reference.rule_epoch {
            return Err(ToolSelectionError::changed());
        }
        if current_epochs.catalog != reference.catalog_epoch
            || current_epochs.acl != reference.acl_epoch
            || revision.schema_hash != reference.schema_hash
        {
            return Err(ToolSelectionError::stale());
        }
        Ok(revision.effect)
    }

    pub async fn invoke(
        &self,
        context: &RequestContext,
        execution_ref: &str,
        arguments: &Value,
        run_cancellation: &RunCancellation,
    ) -> ToolSelectionResult<InvokeResponse> {
        self.invoke_with_origin(
            context,
            execution_ref,
            arguments,
            run_cancellation,
            "conversation",
        )
        .await
    }

    /// Same as [`Self::invoke`] but records the origin that reaches the generated-capability call
    /// row (`conversation` or `mcp`).
    pub async fn invoke_with_origin(
        &self,
        context: &RequestContext,
        execution_ref: &str,
        arguments: &Value,
        run_cancellation: &RunCancellation,
        origin: &'static str,
    ) -> ToolSelectionResult<InvokeResponse> {
        let reference = self
            .references
            .resolve(execution_ref, ReferenceKind::Execution, now_ms())
            .ok_or_else(ToolSelectionError::not_found)?;
        if reference.principal_id != context.principal_id
            || reference.scope_key() != context.scope_key()
        {
            return Err(ToolSelectionError::unauthorized());
        }
        if !arguments.is_object() {
            return Err(ToolSelectionError::invalid());
        }
        let (tool, revision) = self.current_revision(&reference)?;
        let current_epochs = self.read_epochs()?;
        if current_epochs.rule != reference.rule_epoch {
            return Err(ToolSelectionError::changed());
        }
        if current_epochs.catalog != reference.catalog_epoch
            || current_epochs.acl != reference.acl_epoch
        {
            return Err(ToolSelectionError::stale());
        }
        if revision.schema_hash != reference.schema_hash {
            return Err(ToolSelectionError::stale());
        }
        validate_arguments(&revision.input_schema, arguments)?;
        let binding = revision.backend_binding.clone();
        let binding_kind = BackendRouter::kind(&binding);
        // For an external MCP binding the source must still be enabled, freshly synced and
        // matched to the recorded endpoint before an invocation row is even opened.
        if binding_kind == "mcp_http" {
            let parsed = McpBinding::parse(&binding).ok_or_else(ToolSelectionError::stale)?;
            let manager = self
                .mcp_manager
                .clone()
                .ok_or_else(ToolSelectionError::unavailable)?;
            manager
                .preflight(&parsed.source_id, &parsed.endpoint_hash)
                .await
                .map_err(super::mcp::service_support::map_call_error)?;
        }
        let invocation_id = crate::new_id("tsinv");
        let decision_id = reference.decision_id.clone();
        let now = now_ms();
        {
            let invocation_id = invocation_id.clone();
            let revision_id = revision.id.clone();
            write_transaction(&self.writer, move |connection| {
                repository::insert_invocation(
                    connection,
                    &invocation_id,
                    Some(&decision_id),
                    &revision_id,
                    now,
                )
                .map_err(|_| "storage".to_string())
            })
            .map_err(|_| ToolSelectionError::storage())?;
        }

        let request = BackendRequest {
            call_id: invocation_id.clone(),
            tool_id: tool.id.clone(),
            revision_id: revision.id.clone(),
            backend_key: tool.backend_key.clone(),
            binding,
            arguments: arguments.clone(),
            timeout: std::time::Duration::from_millis(BACKEND_TIMEOUT_MS),
            origin,
            actor: Some(crate::generated_capabilities::contracts::CallActor {
                principal_id: context.principal_id.clone(),
                conversation_id: context.conversation_id.clone(),
                project_id: context.project_id.clone(),
                run_id: context
                    .run_id
                    .clone()
                    .unwrap_or_else(|| format!("conversation:{}", context.conversation_id)),
            }),
        };
        // The management task owns the backend call, the cancellation handle, the result storage
        // and the terminal DB write. Dropping this caller future (HTTP disconnect, aborted
        // provider turn) detaches it but does not stop or leak the invocation.
        let receiver = super::invocation::spawn(super::invocation::ManagedInvocation {
            writer: self.writer.clone(),
            backend: self.backend.clone(),
            invocation_id: invocation_id.clone(),
            context: context.clone(),
            tool_id: tool.id.clone(),
            revision: revision.clone(),
            acl_epoch: current_epochs.acl,
            binding_kind: binding_kind.to_string(),
            request,
            cancellation: run_cancellation.clone(),
        });
        match receiver.await {
            Ok(response) => response,
            // The management task panicked before reporting; the row is settled by the
            // crash-reconcile path at the next startup.
            Err(_) => Err(ToolSelectionError::unavailable()),
        }
    }

    /// Resolves one continuation page of a stored MCP result. Ownership, scope, TTL and the
    /// current ACL are re-checked on every read; a revoked or foreign reference is refused.
    pub fn describe_result(
        &self,
        context: &RequestContext,
        result_ref: &str,
        page: i64,
    ) -> ToolSelectionResult<ResultPageResponse> {
        super::mcp::service_support::describe_result(&self.writer, context, result_ref, page)
    }

    /// Development/management API for importing a fixture catalog. Import is never a grant, but
    /// the evaluation fixture grants each tool to the evaluation principal explicitly.
    pub fn ingest_catalog(
        &self,
        principal_id: &str,
        source_id: &str,
        entries: &[CatalogEntry],
    ) -> ToolSelectionResult<usize> {
        let now = now_ms();
        write_transaction(&self.writer, move |connection| {
            for entry in entries {
                let revision_id = format!("{}-rev1", entry.tool_id);
                catalog::register_revision(
                    connection,
                    principal_id,
                    source_id,
                    entry,
                    &revision_id,
                    now,
                )
                .map_err(|error| error.code.as_str().to_string())?;
                repository::upsert_grant(
                    connection,
                    principal_id,
                    &entry.tool_id,
                    "user",
                    principal_id,
                )
                .map_err(|_| "storage".to_string())?;
            }
            repository::bump_epochs(connection, false, true, false)
                .map_err(|_| "storage".to_string())?;
            Ok(entries.len())
        })
        .map_err(|_| ToolSelectionError::storage())
    }

    /// Computes and stores document embeddings for every authorized revision using the configured
    /// embedding provider.
    pub async fn index_embeddings(
        &self,
        principal_id: &str,
        project_id: Option<&str>,
    ) -> ToolSelectionResult<usize> {
        let eligible = self
            .writer
            .read_serialized({
                let principal = principal_id.to_string();
                let project = project_id.map(str::to_string);
                let now = now_ms();
                move |connection| {
                    repository::eligible_revisions(connection, &principal, project.as_deref(), now)
                        .map_err(|error| error.to_string())
                }
            })
            .map_err(|_| ToolSelectionError::storage())?;
        if eligible.is_empty() {
            return Ok(0);
        }
        let texts: Vec<String> = eligible
            .iter()
            .map(|item| retrieval::document_text(&item.revision.search_text))
            .collect();
        // Batch requests so one JSONL response stays well under the 2 MiB line limit.
        let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(EMBED_BATCH_SIZE) {
            let batch = self
                .embedding
                .embed(EmbedKind::Passage, chunk)
                .await
                .map_err(|_| ToolSelectionError::unavailable())?;
            if batch.len() != chunk.len()
                || batch
                    .iter()
                    .any(|vector| vector.len() != self.embedding.dimension())
            {
                return Err(ToolSelectionError::new(
                    ToolSelectionErrorCode::Integrity,
                    "The embedding model returned an unexpected shape.",
                ));
            }
            vectors.extend(batch);
        }
        let model_hash = self.embedding.model_hash().to_string();
        let rows: Vec<(String, Vec<f32>)> = eligible
            .iter()
            .map(|item| item.revision.id.clone())
            .zip(vectors)
            .collect();
        write_transaction(&self.writer, move |connection| {
            for (revision_id, vector) in &rows {
                repository::upsert_embedding(connection, revision_id, &model_hash, vector)
                    .map_err(|_| "storage".to_string())?;
            }
            Ok(rows.len())
        })
        .map_err(|_| ToolSelectionError::storage())
    }

    /// Full stored candidate ranking for a decision, used by the evaluation CLI. The returned
    /// order is the persisted final order before the response limit is applied.
    pub fn decision_candidates(
        &self,
        decision_id: &str,
    ) -> ToolSelectionResult<Vec<CandidateRecord>> {
        self.writer
            .read_serialized(|connection| {
                repository::decision_by_id(connection, decision_id)
                    .map(|decision| decision.map(|value| value.candidates).unwrap_or_default())
                    .map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())
    }

    fn read_epochs(&self) -> ToolSelectionResult<Epochs> {
        self.writer
            .read_serialized(|connection| {
                repository::epochs(connection).map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())
    }
}

/// Returns the local profile UUID used as the tool-selection principal. It is created once in the
/// `tool_selection` settings namespace and never derived from credentials or the OS user name.
pub fn ensure_principal(writer: &SqliteWriter) -> ToolSelectionResult<String> {
    writer
        .write(|connection| {
            let existing: Option<String> = connection
                .query_row(
                    "SELECT value_json FROM settings_documents
                      WHERE namespace = 'tool_selection' AND key = 'principal'",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(crate::database_error)?;
            if let Some(value) = existing {
                if let Ok(serde_json::Value::String(id)) = serde_json::from_str(&value) {
                    if !id.is_empty() {
                        return Ok(id);
                    }
                }
            }
            let id = uuid::Uuid::new_v4().to_string();
            connection
                .execute(
                    "INSERT OR REPLACE INTO settings_documents(
                       namespace, key, schema_version, value_json, updated_at)
                     VALUES ('tool_selection', 'principal', 1, ?1, ?2)",
                    rusqlite::params![
                        serde_json::Value::String(id.clone()).to_string(),
                        crate::now_iso()
                    ],
                )
                .map_err(crate::database_error)?;
            Ok(id)
        })
        .map_err(|_| ToolSelectionError::storage())
}

const CHANGED: &str = "selection-changed";

enum PersistError {
    Storage,
    Changed,
}

/// Settles any invocation left `running` by a crash so the ledger never reports a call as still
/// in flight after a restart. Returns the number of rows repaired.
pub fn reconcile_interrupted_invocations(writer: &SqliteWriter) -> ToolSelectionResult<usize> {
    writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE tool_selection_invocations
                        SET technical_status = 'interrupted',
                            finished_at = ?1,
                            error_code = CASE
                              WHEN error_code IS NULL AND EXISTS (
                                SELECT 1 FROM tool_selection_revisions r
                                 WHERE r.id = revision_id
                                   AND json_extract(r.backend_binding_json, '$.kind') = 'mcp_http')
                              THEN 'remote-outcome-unknown'
                              ELSE error_code END
                      WHERE technical_status = 'running'",
                    rusqlite::params![now_ms()],
                )
                .map_err(crate::database_error)
        })
        .map_err(|_| ToolSelectionError::storage())
}

fn write_transaction<T>(
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

fn title_and_summary(search_text: &str) -> (String, String) {
    let mut title = String::new();
    let mut purpose = String::new();
    for line in search_text.lines() {
        if title.is_empty() {
            if let Some(rest) = line.strip_prefix("title: ") {
                title = rest.to_string();
            }
        }
        if purpose.is_empty() {
            if let Some(rest) = line.strip_prefix("purpose: ") {
                purpose = rest.to_string();
            }
        }
    }
    if title.is_empty() {
        title = "tool".to_string();
    }
    let summary =
        repository::truncate_utf8(&purpose, SEARCH_CANDIDATE_SUMMARY_MAX_BYTES).to_string();
    (title, summary)
}

fn validate_arguments(schema: &Value, arguments: &Value) -> ToolSelectionResult<()> {
    let bytes = serde_json::to_vec(arguments).map_err(|_| ToolSelectionError::invalid())?;
    if bytes.len() > BACKEND_INPUT_MAX_BYTES {
        return Err(ToolSelectionError::invalid());
    }
    let validator = jsonschema::validator_for(schema).map_err(|_| ToolSelectionError::invalid())?;
    if validator.is_valid(arguments) {
        Ok(())
    } else {
        Err(ToolSelectionError::invalid())
    }
}

fn encode_cursor(revision_id: &str, section: &str, page: i64) -> String {
    let raw = json!({ "r": revision_id, "s": section, "p": page }).to_string();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes())
}

fn decode_cursor(
    cursor: Option<&str>,
    revision_id: &str,
    section: &str,
) -> ToolSelectionResult<i64> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| ToolSelectionError::invalid())?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ToolSelectionError::invalid())?;
    let bound_revision = value.get("r").and_then(Value::as_str);
    let bound_section = value.get("s").and_then(Value::as_str);
    if bound_revision != Some(revision_id) || bound_section != Some(section) {
        return Err(ToolSelectionError::not_found());
    }
    let page = value
        .get("p")
        .and_then(Value::as_i64)
        .filter(|page| *page >= 0)
        .ok_or_else(ToolSelectionError::invalid)?;
    Ok(page)
}
