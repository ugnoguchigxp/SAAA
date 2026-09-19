//! Orchestration for the three entry points plus correction ingestion. Reads snapshot the ledger
//! and release the writer lock before any inference; writes re-check epochs inside one immediate
//! transaction so a rule or ACL change cannot be published behind an older selection.

#![allow(private_interfaces)]

use base64::Engine;
use rusqlite::{Connection, TransactionBehavior};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use super::backends::{BackendRequest, TechnicalStatus, ToolBackend};
use super::contracts::*;
use super::extraction::{CorrectionExtractor, ExtractionRequest, RecentDecision};
use super::feedback::{apply_extraction, ParsedExtraction};
use super::inference::{EmbedKind, EmbeddingProvider, RerankProvider};
use super::references::{ReferenceEntry, ReferenceKind, ReferenceStore};
use super::repository::{self, Epochs, EligibleRevision};
use super::{ranking, retrieval, rules};
use crate::persistence::SqliteWriter;
use crate::RunCancellation;

const SEARCH_CANDIDATE_POOL: usize = 50;
const BACKEND_TIMEOUT_MS: u64 = 30_000;
const MAX_SEARCH_ATTEMPTS: usize = 2;

#[derive(Clone, Debug)]
pub struct SearchCandidate {
    pub reference: String,
    pub revision_id: String,
    pub tool_id: String,
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
}

pub struct ToolSelectionService {
    writer: Arc<SqliteWriter>,
    references: ReferenceStore,
    embedding: Arc<dyn EmbeddingProvider>,
    reranker: Arc<dyn RerankProvider>,
    extractor: Arc<dyn CorrectionExtractor>,
    backend: Arc<dyn ToolBackend>,
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
            no_match_threshold,
            scenarios: Mutex::new(HashMap::new()),
            discovery_configured: true,
        }
    }

    pub fn references(&self) -> &ReferenceStore {
        &self.references
    }

    pub fn set_discovery_configured(&mut self, configured: bool) {
        self.discovery_configured = configured;
    }

    pub fn discovery_configured(&self) -> bool {
        self.discovery_configured
    }

    pub fn set_scenario(&self, context: &RequestContext, scenario: Scenario) {
        if let Ok(mut scenarios) = self.scenarios.lock() {
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

    pub async fn begin_turn(&self, context: &RequestContext, user_message: &str) -> TurnOutcome {
        let intent = repository::truncate_utf8(user_message, EXTRACT_USER_MESSAGE_MAX_BYTES);
        let (allowed_decisions, allowed_tools, recent) = self.allowed_for_extraction(context);
        let request = ExtractionRequest {
            user_message: intent.to_string(),
            recent_decisions: recent,
            allowed_decisions,
            allowed_tools,
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
    ) -> (HashSet<String>, HashSet<String>, Vec<RecentDecision>) {
        let principal = context.principal_id.clone();
        let conversation = context.conversation_id.clone();
        let project = context.project_id.clone();
        self.writer
            .read_serialized(|connection| {
                let eligible = repository::eligible_revisions(
                    connection,
                    &principal,
                    project.as_deref(),
                )
                .map_err(|error| error.to_string())?;
                let mut tools: HashSet<String> = HashSet::new();
                for item in &eligible {
                    tools.insert(item.revision.tool_id.clone());
                    if let Ok(Some(tool)) =
                        repository::tool_by_id(connection, &item.revision.tool_id)
                    {
                        tools.insert(tool.backend_key);
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
                for (decision_id, scenario, tool_ids) in decisions {
                    allowed_decisions.insert(decision_id.clone());
                    for tool_id in &tool_ids {
                        tools.insert(tool_id.clone());
                    }
                    recent.push(RecentDecision {
                        decision_id,
                        scenario_summary: scenario,
                        tool_ids,
                    });
                }
                Ok((allowed_decisions, tools, recent))
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
        let query = intent.to_string();
        let now = now_ms();
        self.writer
            .read_serialized(|connection| {
                let epochs = repository::epochs(connection).map_err(|error| error.to_string())?;
                let eligible = repository::eligible_revisions(
                    connection,
                    &principal,
                    project.as_deref(),
                )
                .map_err(|error| error.to_string())?;
                let lexical = if lexical_ok {
                    repository::lexical_candidates(
                        connection,
                        &query,
                        &principal,
                        project.as_deref(),
                        SEARCH_CANDIDATE_POOL,
                    )
                    .map_err(|error| error.to_string())?
                    .into_iter()
                    .map(|(revision_id, _)| revision_id)
                    .collect()
                } else {
                    Vec::new()
                };
                let embeddings = repository::load_embeddings(
                    connection,
                    &model_hash,
                    &principal,
                    project.as_deref(),
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
        let mut vector_degraded = self.embedding.dimension() == 0;
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
                    let scored = retrieval::embedding_candidates(
                        &vectors[0],
                        &snapshot.embeddings,
                    );
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

        let mut fused = retrieval::fuse_candidates(&snapshot.lexical, &vector_ids, RERANK_TOP);
        // Inject explicitly preferred tools that retrieval missed, authorized only.
        let mut injected = 0;
        for rule in rules::dedupe_by_condition(&snapshot.rules) {
            if injected >= RERANK_PREFERRED_MAX {
                break;
            }
            if rule.action != RuleAction::Prefer {
                continue;
            }
            if !rules::scope_matches(rule, context) || !rules::condition_matches(rule, scenario) {
                continue;
            }
            let Some(tool_id) = rule.target_tool_id.as_deref() else {
                continue;
            };
            if fused.iter().any(|item| item.revision_id == tool_id) {
                continue;
            }
            if let Some(revision_id) = tool_revision.get(tool_id) {
                if !fused
                    .iter()
                    .any(|item| item.revision_id == *revision_id)
                {
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
                            retrieval::rerank_document(&revision.revision.search_text),
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
                revision_by_id.get(revision_id.as_str()).map(|revision| {
                    rules::BaseCandidate {
                        revision_id: revision_id.clone(),
                        tool_id: revision.revision.tool_id.clone(),
                        base_score: ranking::base_from_rank(index + 1, total),
                    }
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
        Ok(RankOutcome {
            ordered: correction.ordered,
            ranks,
            raw,
            status,
            degraded: vector_degraded || rerank_degraded,
            notes,
        })
    }

    pub async fn search(
        &self,
        context: &RequestContext,
        intent: &str,
        limit: usize,
    ) -> ToolSelectionResult<SearchResponse> {
        let intent = intent.trim();
        if intent.is_empty() || intent.len() > SEARCH_INTENT_MAX_BYTES {
            return Err(ToolSelectionError::invalid());
        }
        if limit < 1 || limit > SEARCH_LIMIT_MAX {
            return Err(ToolSelectionError::invalid());
        }
        let scenario = self.cached_scenario(context, intent);
        let mut outcome = None;
        for _ in 0..MAX_SEARCH_ATTEMPTS {
            let snapshot = self.snapshot(context, intent).await?;
            let ranking = self.rank(context, &scenario, intent, &snapshot).await?;
            let decision_id = crate::new_id("tsdecision");
            match self.persist(context, &scenario, &decision_id, &snapshot, &ranking) {
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
            candidates.push(SearchCandidate {
                reference,
                revision_id: candidate.revision_id.clone(),
                tool_id: candidate.tool_id.clone(),
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
            repository::insert_decision(connection, &decision).map_err(|_| "storage".to_string())
        })
        .map_err(|error| {
            if error == CHANGED {
                PersistError::Changed
            } else {
                PersistError::Storage
            }
        })
    }

    fn read_revision(&self, revision_id: &str) -> ToolSelectionResult<Option<repository::RevisionRow>> {
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
        if !matches!(section, "contract" | "usage" | "examples" | "troubleshooting") {
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
        let execution_ref = self.issue_execution_ref(context, &reference, &revision)?;
        if section == "contract" {
            let body = json!({
                "revisionId": revision.id,
                "title": tool.backend_key,
                "inputSchema": revision.input_schema,
                "outputSchema": revision.output_schema,
            });
            if serde_json::to_vec(&body).map(|bytes| bytes.len()).unwrap_or(usize::MAX)
                > DESCRIBE_RESPONSE_MAX_BYTES
            {
                return Err(ToolSelectionError::new(
                    ToolSelectionErrorCode::Unavailable,
                    "The tool contract is too large to describe.",
                ));
            }
            return Ok(DescribeResponse {
                revision_id: revision.id,
                tool_id: revision.tool_id,
                section: section.to_string(),
                body,
                execution_ref: Some(execution_ref),
                cursor: None,
            });
        }
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
        let result = self
            .writer
            .read_serialized(move |connection| {
                let tool = repository::tool_by_id(connection, &tool_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "not-found".to_string())?;
                if !tool.enabled || tool.current_revision_id.as_deref() != Some(revision_id.as_str())
                {
                    return Err("stale".to_string());
                }
                let authorized = repository::grant_exists(
                    connection,
                    &principal,
                    &tool_id,
                    project.as_deref(),
                )
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

    pub async fn invoke(
        &self,
        context: &RequestContext,
        execution_ref: &str,
        arguments: &Value,
        run_cancellation: &RunCancellation,
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
        };
        let outcome = self.backend.invoke(request, run_cancellation).await;
        let mut status = outcome.status;
        let mut error_code = outcome.error_code;
        let mut result = outcome.result;
        if let Some(value) = &result {
            let too_large = serde_json::to_vec(value)
                .map(|bytes| bytes.len() > BACKEND_RESULT_MAX_BYTES)
                .unwrap_or(true);
            if too_large {
                result = None;
                status = TechnicalStatus::Failed;
                error_code = Some("output-limit");
            }
        }
        let finished = now_ms();
        {
            let invocation_id = invocation_id.clone();
            write_transaction(&self.writer, move |connection| {
                repository::finish_invocation(
                    connection,
                    &invocation_id,
                    status.as_str(),
                    error_code,
                    finished,
                )
                .map_err(|_| "storage".to_string())
            })
            .map_err(|_| ToolSelectionError::storage())?;
        }
        if status == TechnicalStatus::Cancelled {
            return Err(ToolSelectionError::new(
                ToolSelectionErrorCode::Cancelled,
                "The tool call was cancelled.",
            ));
        }
        Ok(InvokeResponse {
            invocation_id,
            status,
            result,
            error_code,
        })
    }

    fn read_epochs(&self) -> ToolSelectionResult<Epochs> {
        self.writer
            .read_serialized(|connection| {
                repository::epochs(connection).map_err(|error| error.to_string())
            })
            .map_err(|_| ToolSelectionError::storage())
    }
}

const CHANGED: &str = "selection-changed";

enum PersistError {
    Storage,
    Changed,
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
    let summary = repository::truncate_utf8(&purpose, SEARCH_CANDIDATE_SUMMARY_MAX_BYTES).to_string();
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

/// Kept public for the gateway so the model-facing envelope can be built without leaking details.
pub fn error_envelope(error: &ToolSelectionError) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": error.code.as_str(),
            "message": error.message,
            "retryable": error.retryable,
        }
    })
}
