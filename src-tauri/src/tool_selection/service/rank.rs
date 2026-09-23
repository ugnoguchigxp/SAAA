use super::*;
impl ToolSelectionService {
    pub(super) async fn rank(
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
}
impl ToolSelectionService {
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
}
impl ToolSelectionService {
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
}
impl ToolSelectionService {
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
            let (source_id, source_label) = super::super::mcp::service_support::source_display(
                &self.writer,
                &candidate.tool_id,
            );
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
}
impl ToolSelectionService {
    pub(super) fn persist(
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
}
impl ToolSelectionService {
    pub(super) fn read_revision(
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
}
