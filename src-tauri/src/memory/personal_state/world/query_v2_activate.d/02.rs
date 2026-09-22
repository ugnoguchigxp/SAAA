/// D32: complete the v2 query. Pure exploration over the loaded context plus
/// Focus/Gap selection, whole-unit byte budget and flags.
pub fn activate_v2_with_stats(
    c: &Connection,
    input: &ActivateInputV2<'_>,
) -> Result<(WorldSliceV2, QueryStatsV2), String> {
    let observation_sources: BTreeSet<_> = input
        .condition_observations
        .iter()
        .map(|observation| observation.source.clone())
        .chain(
            input
                .availability_observations
                .iter()
                .map(|observation| observation.source.clone()),
        )
        .collect();
    let ledger = store::load_world_query(c, input.project_scope, &observation_sources)?;
    let context = super::load_query_context_v2(c, input, &ledger)?;
    let limits = input.limits.capped();
    let max_bytes = input.max_bytes.min(WorldSliceV2::MAX_BYTES);
    let mut slice = WorldSliceV2::empty(context.revision, input.now);
    slice.notices.extend(context.notices.clone());
    // A disabled kind is an explicit omission reason, never knowledge loss.
    for (enabled, name) in [
        (input.flags.causal, "causal"),
        (input.flags.goals, "goals"),
        (input.flags.correlations, "correlations"),
        (input.flags.dependencies, "dependencies"),
    ] {
        if !enabled {
            push_notice(&mut slice, &format!("disabled:{name}"));
        }
    }
    if context.stale || context.pending {
        return assemble_slice(slice, &[], max_bytes)
            .map(|slice| (slice, QueryStatsV2::default()))
            .map_err(|e| e.code().to_string());
    }
    if context.seeds.is_empty() {
        // An unresolved (or ambiguous) explicit seed yields a missing_knowledge
        // Gap, never a fabricated node or edge. A non-explicit empty seed is an
        // empty envelope with the unknown_seed notice already in `slice`.
        let mut units: Vec<SliceUnitV2> = Vec::new();
        if input.explicit_question {
            let unresolved = input.seeds.iter().map(seed_text).next();
            if let Some(seed) = unresolved {
                let names = BTreeMap::new();
                let gaps = build_gap_candidates(&GapInputV2 {
                    project_scope: input.project_scope,
                    names: &names,
                    relations: &[],
                    outcomes: &[],
                    unknown_availability: &[],
                    unresolved_seed: Some(&seed),
                    explicit_question: true,
                });
                units.push(SliceUnitV2 {
                    research_gaps: gaps,
                    ..Default::default()
                });
            }
        }
        return assemble_slice(slice, &units, max_bytes)
            .map(|slice| (slice, QueryStatsV2::default()))
            .map_err(|e| e.code().to_string());
    }

    let load = load_edges_bfs(c, input, &context)?;
    let stats = QueryStatsV2 {
        fetch_rows: load.fetched,
        scan_steps: load.scanned,
    };
    if load.truncated {
        if let Some(reason) = load.reason {
            push_reason(&mut slice, &format!("truncated:{reason}"));
        }
    }
    let edges = &load.edges;
    let related = traverse_v2(
        edges,
        &context.seeds,
        limits,
        TraversalModeV2::Related,
        CausalDirection::Forward,
    );
    let causal = traverse_v2(
        edges,
        &context.seeds,
        limits,
        TraversalModeV2::Causal,
        input.causal_direction,
    );

    // Focus candidates: persisted Focus plus request-only temporary attention.
    // With `goals=false` a Goal Focus is omitted, so no goal node or goal-derived
    // Gap is returned.
    let goal_allowed = |entity_id: &str| {
        input.flags.goals
            || context
                .entities
                .get(entity_id)
                .is_some_and(|row| row.kind != EntityKindV2::Goal)
    };
    let mut focus_candidates: Vec<FocusCandidate> = context
        .focus
        .iter()
        .filter(|row| context.entities.contains_key(&row.entity_id) && goal_allowed(&row.entity_id))
        .map(|row| FocusCandidate {
            entity_id: row.entity_id.clone(),
            reason: row.reason.clone(),
            objective_assertion_id: row.objective_assertion_id.clone(),
            distance: distance_from(&related, &context.seeds, &row.entity_id),
            assertion_id: row.assertion_id.clone(),
        })
        .collect();
    if input.flags.goals {
        for entity_id in input.temporary_attention_entity_ids {
            if context.entities.contains_key(entity_id) && goal_allowed(entity_id) {
                focus_candidates.push(temporary_attention_focus(entity_id, input.request_id));
            }
        }
    }
    let focus_candidates = order_focus(focus_candidates);
    let focus_ids: BTreeSet<String> = focus_candidates
        .iter()
        .map(|candidate| candidate.entity_id.clone())
        .collect();

    // Relevance paths must terminate at a Focus entity.
    let mut relevance_paths: Vec<WorldPathV2> = related
        .paths
        .iter()
        .filter(|path| {
            path.nodes
                .last()
                .is_some_and(|node| focus_ids.contains(node))
        })
        .cloned()
        .collect();
    let mut causal_paths: Vec<WorldPathV2> = maximal_only_v2(&causal.paths);

    let mut budget_reason: Option<&'static str> = None;
    let total_paths = relevance_paths.len() + causal_paths.len();
    if total_paths > limits.paths {
        let keep_relevance = relevance_paths.len().min(limits.paths);
        relevance_paths.truncate(keep_relevance);
        causal_paths.truncate(limits.paths - keep_relevance);
        budget_reason = Some("paths");
    }

    // Gaps from the exact (Focus, edge) pairs on retained relevance paths.
    let mut gap_relations: Vec<GapRelationV2> = Vec::new();
    for path in &relevance_paths {
        let Some(focus_id) = path.nodes.last() else {
            continue;
        };
        let rank = focus_candidates
            .iter()
            .find(|candidate| &candidate.entity_id == focus_id)
            .map(|candidate| focus_rank_v2(&candidate.reason))
            .unwrap_or(3);
        for step in &path.steps {
            let edge = &edges[step.edge];
            gap_relations.push(GapRelationV2 {
                focus_entity_id: Some(focus_id.clone()),
                focus_rank: rank,
                path_len: path.steps.len(),
                edge: edge.clone(),
                evidence: Vec::new(),
            });
        }
    }
    let namespace: BTreeMap<String, String> = context
        .entities
        .iter()
        .map(|(id, row)| (id.clone(), row.name.clone()))
        .collect();

    // Dependencies on dependency paths.
    let dep_paths = traverse_v2(
        edges,
        &context.seeds,
        limits,
        TraversalModeV2::Dependencies,
        CausalDirection::Forward,
    );
    let mut dep_edges: BTreeSet<usize> = BTreeSet::new();
    for path in &dep_paths.paths {
        for step in &path.steps {
            dep_edges.insert(step.edge);
        }
    }
    let availability: BTreeMap<
        String,
        (
            AvailabilityStateV2,
            Vec<saaa_personal_state_core::SourceKey>,
        ),
    > = edges
        .iter()
        .filter(|edge| edge.relation_type == RelationTypeV2::DependsOn)
        .map(|edge| {
            let evaluation = evaluate_availability(&edge.to, &context.observations.availability);
            (edge.to.clone(), (evaluation.state, evaluation.evidence))
        })
        .collect();
    let dependency_rows = dependencies(edges, &dep_edges, &availability);
    let unknown_availability: Vec<UnknownAvailabilityV2> = dependency_rows
        .iter()
        .filter(|row| row.availability == AvailabilityStateV2::Unknown)
        .map(|row| UnknownAvailabilityV2 {
            relation_id: row.relation_id.clone(),
            focus_entity_id: None,
            focus_rank: 3,
            path_len: 1,
            evidence: row.evidence.clone(),
        })
        .collect();

    // Seeds resolved in the main path, so there is no unresolved-seed Gap here.
    let unresolved_seed: Option<String> = None;
    let gaps = build_gap_candidates(&GapInputV2 {
        project_scope: input.project_scope,
        names: &namespace,
        relations: &gap_relations,
        outcomes: &Vec::<OutcomeConflictV2>::new(),
        unknown_availability: &unknown_availability,
        unresolved_seed: unresolved_seed.as_deref(),
        explicit_question: input.explicit_question,
    });

    // Whole-path explanation units.
    let mut units: Vec<SliceUnitV2> = Vec::new();
    // The seed itself is always explained ("depth 0"): a seed with no edges must
    // still return its node and, if applicable, its Focus.
    for seed in &context.seeds {
        if !goal_allowed(seed) {
            continue;
        }
        if let Some(entity) = context.entities.get(seed) {
            let node = node_of(entity);
            let focus = focus_candidates
                .iter()
                .find(|candidate| &candidate.entity_id == seed)
                .map(slice_focus_of)
                .into_iter()
                .collect();
            units.push(SliceUnitV2 {
                relevant_goal_ids: goal_ids(std::slice::from_ref(&node)),
                relevant_project_ids: project_ids(std::slice::from_ref(&node)),
                nodes: vec![node],
                focus,
                ..Default::default()
            });
        }
    }
    let mut path_edge_indices: BTreeSet<usize> = BTreeSet::new();
    for path in &relevance_paths {
        for step in &path.steps {
            path_edge_indices.insert(step.edge);
        }
        let focus_id = path.nodes.last().cloned();
        let goal_id = focus_id
            .as_deref()
            .and_then(|id| context.entities.get(id))
            .filter(|row| row.kind == EntityKindV2::Goal)
            .and_then(|row| row.objective_assertion_id.clone());
        let Some(view) = path_view(path, edges, &context, focus_id.as_deref(), goal_id, false)
        else {
            continue;
        };
        let relevant_gaps: Vec<ResearchGapV2> = gaps
            .iter()
            .filter(|gap| gap.focus_entity_id == focus_id)
            .cloned()
            .collect();
        let focus_dto: Vec<SliceFocusV2> = focus_id
            .as_deref()
            .and_then(|id| {
                focus_candidates
                    .iter()
                    .find(|candidate| candidate.entity_id == id)
            })
            .map(slice_focus_of)
            .into_iter()
            .collect();
        let goals = goal_ids(&view.nodes);
        let projects = project_ids(&view.nodes);
        units.push(SliceUnitV2 {
            nodes: view.nodes,
            relations: view.relations,
            focus: focus_dto,
            relevance_paths: view.relevance.into_iter().collect(),
            research_gaps: relevant_gaps,
            relevant_goal_ids: goals,
            relevant_project_ids: projects,
            ..Default::default()
        });
    }
    for path in &causal_paths {
        for step in &path.steps {
            path_edge_indices.insert(step.edge);
        }
        let reverse = input.causal_direction == CausalDirection::Reverse;
        let Some(view) = path_view(path, edges, &context, None, None, reverse) else {
            continue;
        };
        let goals = goal_ids(&view.nodes);
        let projects = project_ids(&view.nodes);
        units.push(SliceUnitV2 {
            nodes: view.nodes,
            relations: view.relations,
            causal_paths: view.causal.into_iter().collect(),
            relevant_goal_ids: goals,
            relevant_project_ids: projects,
            ..Default::default()
        });
    }
    if let Some(seed_gap) = gaps.iter().find(|gap| {
        gap.kind == GapKindV2::MissingKnowledge && gap.subject.unresolved_seed.is_some()
    }) {
        units.push(SliceUnitV2 {
            research_gaps: vec![seed_gap.clone()],
            ..Default::default()
        });
    }

    // Standalone correlation / dependency units.
    for (index, edge) in edges.iter().enumerate() {
        if path_edge_indices.contains(&index) {
            continue;
        }
        match edge.relation_type {
            RelationTypeV2::CorrelatesWith if input.flags.correlations => {
                let (Some(from), Some(to)) = (
                    context.entities.get(&edge.from),
                    context.entities.get(&edge.to),
                ) else {
                    continue;
                };
                let relevant_gaps: Vec<ResearchGapV2> = gaps
                    .iter()
                    .filter(|gap| gap.subject.relation_ids.contains(&edge.assertion_id))
                    .cloned()
                    .collect();
                units.push(SliceUnitV2 {
                    nodes: vec![node_of(from), node_of(to)],
                    relations: vec![relation_to_slice(edge, &context.observations)],
                    correlation_ids: vec![edge.assertion_id.clone()],
                    research_gaps: relevant_gaps,
                    ..Default::default()
                });
            }
            RelationTypeV2::DependsOn if input.flags.dependencies => {
                let (Some(from), Some(to)) = (
                    context.entities.get(&edge.from),
                    context.entities.get(&edge.to),
                ) else {
                    continue;
                };
                let evaluation =
                    evaluate_availability(&edge.to, &context.observations.availability);
                units.push(SliceUnitV2 {
                    nodes: vec![node_of(from), node_of(to)],
                    relations: vec![relation_to_slice(edge, &context.observations)],
                    dependencies: vec![DependencyV2 {
                        relation_id: edge.assertion_id.clone(),
                        required_entity_id: edge.to.clone(),
                        availability: evaluation.state,
                        evidence: evaluation.evidence,
                    }],
                    ..Default::default()
                });
            }
            _ => {}
        }
    }

    // Derived correlation IDs and effect summaries are added after the byte
    // budget has selected the final paths. Building them from candidate paths
    // here would leave dangling indices or prune them before their path unit is
    // adopted.
    // Focus is part of the envelope budget: one unit per reachable Focus so a
    // Focus whose entity exists but is not on a retained path is still shown
    // (and trimmed as a whole unit if it does not fit).
    for candidate in &focus_candidates {
        if let Some(entity) = context.entities.get(&candidate.entity_id) {
            let node = node_of(entity);
            units.push(SliceUnitV2 {
                focus: vec![slice_focus_of(candidate)],
                relevant_goal_ids: goal_ids(std::slice::from_ref(&node)),
                relevant_project_ids: project_ids(std::slice::from_ref(&node)),
                nodes: vec![node],
                ..Default::default()
            });
        }
    }
    if budget_reason.is_some() {
        push_reason(&mut slice, "truncated:paths");
    }
    for reason in load
        .truncation_reasons()
        .into_iter()
        .chain(budget_reason.into_iter())
    {
        push_reason(&mut slice, &format!("truncated:{reason}"));
    }

    // Keep room for a truncation marker if metadata derived from the selected
    // paths does not fit. The empty sentinel otherwise changes no slice data.
    units.push(SliceUnitV2::default());
    let assembled = assemble_slice(slice, &units, max_bytes).map_err(|e| e.code().to_string())?;
    let metadata = derived_metadata_unit(&assembled, !load.truncated);
    assemble_slice(assembled, &[metadata], max_bytes)
        .map(|slice| (slice, stats))
        .map_err(|e| e.code().to_string())
}
fn derived_metadata_unit(slice: &WorldSliceV2, source_complete: bool) -> SliceUnitV2 {
    type SummaryKey = (String, String, Option<String>);
    let relation_by_id: BTreeMap<&str, &SliceRelationV2> = slice
        .relations
        .iter()
        .map(|relation| (relation.assertion_id.as_str(), relation))
        .collect();
    let correlation_ids = slice
        .relations
        .iter()
        .filter(|relation| relation.relation_type == RelationTypeV2::CorrelatesWith)
        .map(|relation| relation.assertion_id.clone())
        .collect();
    let mut groups: BTreeMap<SummaryKey, Vec<(usize, EffectDirection)>> = BTreeMap::new();
    for (index, path) in slice.causal_paths.iter().enumerate() {
        let (Some(from), Some(to), Some(first), Some(last)) = (
            path.node_ids.first(),
            path.node_ids.last(),
            path.steps.first(),
            path.steps.last(),
        ) else {
            continue;
        };
        let comparison = relation_by_id
            .get(first.assertion_id.as_str())
            .and_then(|relation| relation.comparison_id.clone())
            .or_else(|| {
                relation_by_id
                    .get(last.assertion_id.as_str())
                    .and_then(|relation| relation.comparison_id.clone())
            });
        groups
            .entry((from.clone(), to.clone(), comparison))
            .or_default()
            .push((index, path.direction));
    }
    let complete = source_complete && slice.truncated.is_empty();
    let effect_summaries = groups
        .into_iter()
        .map(|((from_entity_id, to_entity_id, comparison_id), entries)| {
            let directions: BTreeSet<_> = entries
                .iter()
                .map(|(_, direction)| direction.as_str())
                .collect();
            EffectSummaryV2 {
                from_entity_id,
                to_entity_id,
                comparison_id,
                direction: if directions.len() > 1 {
                    EffectDirection::Mixed
                } else {
                    entries
                        .first()
                        .map(|(_, direction)| *direction)
                        .unwrap_or(EffectDirection::Unknown)
                },
                path_indices: entries.into_iter().map(|(index, _)| index).collect(),
                complete,
            }
        })
        .collect();
    SliceUnitV2 {
        correlation_ids,
        effect_summaries,
        ..Default::default()
    }
}
impl EdgeLoad {
    fn truncation_reasons(&self) -> Vec<&'static str> {
        self.reason.into_iter().collect()
    }
}
/// Compatibility entry used by the plan: `activate_v2` returns only the Slice.
pub fn activate_v2(c: &Connection, input: &ActivateInputV2<'_>) -> Result<WorldSliceV2, String> {
    activate_v2_with_stats(c, input).map(|(slice, _)| slice)
}
fn distance_from(
    traversal: &saaa_personal_state_core::world::traversal_v2::TraversalV2Outcome,
    seeds: &[String],
    target: &str,
) -> usize {
    if seeds.iter().any(|seed| seed == target) {
        return 0;
    }
    traversal
        .paths
        .iter()
        .filter(|path| path.nodes.last().is_some_and(|node| node == target))
        .map(|path| path.steps.len())
        .min()
        .unwrap_or(usize::MAX)
}
fn push_notice(slice: &mut WorldSliceV2, notice: &str) {
    if !slice.notices.iter().any(|existing| existing == notice) {
        slice.notices.push(notice.to_string());
    }
}
fn push_reason(slice: &mut WorldSliceV2, reason: &str) {
    if !slice.truncated.iter().any(|existing| existing == reason) {
        slice.truncated.push(reason.to_string());
    }
}
