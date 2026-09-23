use super::*;
pub(super) struct EdgeLoad {
    pub(super) edges: Vec<WorldEdgeV2>,
    pub(super) fetched: usize,
    pub(super) scanned: usize,
    pub(super) truncated: bool,
    pub(super) reason: Option<&'static str>,
}
pub(super) fn flags_allow(flags: IncludeFlags, relation_type: RelationTypeV2) -> bool {
    if !flags.correlations && relation_type == RelationTypeV2::CorrelatesWith {
        return false;
    }
    if !flags.dependencies && relation_type == RelationTypeV2::DependsOn {
        return false;
    }
    if !flags.goals
        && matches!(
            relation_type,
            RelationTypeV2::HasGoal | RelationTypeV2::ServesGoal
        )
    {
        return false;
    }
    if !flags.causal && relation_type.is_signed_effect() {
        return false;
    }
    true
}
pub(super) fn allowed_relation_types(flags: IncludeFlags) -> Vec<&'static str> {
    [
        RelationTypeV2::RelatedTo,
        RelationTypeV2::PartOf,
        RelationTypeV2::DependsOn,
        RelationTypeV2::ImportantFor,
        RelationTypeV2::Increases,
        RelationTypeV2::Decreases,
        RelationTypeV2::Causes,
        RelationTypeV2::Enables,
        RelationTypeV2::Inhibits,
        RelationTypeV2::HasGoal,
        RelationTypeV2::ServesGoal,
        RelationTypeV2::CorrelatesWith,
    ]
    .into_iter()
    .filter(|relation_type| flags_allow(flags, *relation_type))
    .map(|relation_type| relation_type.as_str())
    .collect()
}
/// D31: breadth-first fetch. Each depth reads the relations touching the
/// frontier, with the SQL LIMIT set to the remaining fetch budget. Both fetch
/// and expansion costs are counted; `LIMIT + 1` is never used.
pub(super) fn load_edges_bfs(
    c: &Connection,
    input: &ActivateInputV2<'_>,
    context: &QueryContextV2,
) -> Result<EdgeLoad, String> {
    let limits = input.limits.capped();
    let max_depth = limits.causal_depth.max(limits.relevance_depth);
    let mut load = EdgeLoad {
        edges: Vec::new(),
        fetched: 0,
        scanned: 0,
        truncated: false,
        reason: None,
    };
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut expanded: BTreeSet<String> = BTreeSet::new();
    let mut frontier: Vec<String> = context.seeds.clone();
    frontier.sort();
    frontier.dedup();

    // `max_depth` expansion rounds produce edges at distances 1..=max_depth.
    for _depth in 0..max_depth {
        let active: Vec<String> = frontier
            .iter()
            .filter(|id| !expanded.contains(*id))
            .cloned()
            .collect();
        if active.is_empty() {
            break;
        }
        for id in &active {
            expanded.insert(id.clone());
        }
        let remaining = limits.fetch_rows.saturating_sub(load.fetched);
        if remaining == 0 {
            load.truncated = true;
            load.reason.get_or_insert("fetch");
            break;
        }
        let frontier_json =
            serde_json::to_string(&active).map_err(|_| "world-projection-corrupt")?;
        let permitted_json =
            serde_json::to_string(&context.permitted).map_err(|_| "world-projection-corrupt")?;
        let entity_ids: Vec<&String> = context.entities.keys().collect();
        let entity_ids_json =
            serde_json::to_string(&entity_ids).map_err(|_| "world-projection-corrupt")?;
        let allowed_types_json = serde_json::to_string(&allowed_relation_types(input.flags))
            .map_err(|_| "world-projection-corrupt")?;
        let seen_json = serde_json::to_string(&seen).map_err(|_| "world-projection-corrupt")?;
        let mut statement = c
            .prepare(
                "SELECT r.assertion_id,r.from_entity_id,r.to_entity_id,r.relation_type,r.status,
                        json_extract(a.metadata,'$.semantic_key'),p.value_json
                   FROM personal_world_relations r
                   JOIN personal_assertions a ON a.id=r.assertion_id
                   JOIN personal_payloads p ON p.id=a.payload_id
                  WHERE r.project_scope=:project AND r.valid_from<=:now
                    AND (r.valid_until IS NULL OR :now<r.valid_until)
                    AND r.assertion_id IN (SELECT value FROM json_each(:permitted))
                    AND r.assertion_id NOT IN (SELECT value FROM json_each(:seen))
                    AND r.from_entity_id IN (SELECT value FROM json_each(:entities))
                    AND r.to_entity_id IN (SELECT value FROM json_each(:entities))
                    AND r.relation_type IN (SELECT value FROM json_each(:allowed_types))
                    AND (r.from_entity_id IN (SELECT value FROM json_each(:frontier))
                         OR r.to_entity_id IN (SELECT value FROM json_each(:frontier)))
                  ORDER BY r.assertion_id
                  LIMIT :limit",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map(
                named_params! {
                    ":project": input.project_scope,
                    ":now": input.now,
                    ":frontier": frontier_json,
                    ":permitted": permitted_json,
                    ":seen": seen_json,
                    ":entities": entity_ids_json,
                    ":allowed_types": allowed_types_json,
                    ":limit": remaining as i64,
                },
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, String>(6)?,
                    ))
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        let fetched = rows.len();
        load.fetched += fetched;
        load.scanned += fetched;
        if fetched == remaining {
            load.truncated = true;
            load.reason.get_or_insert("fetch");
        }
        let mut next_frontier: Vec<String> = Vec::new();
        for (assertion_id, from, to, _relation_type, status, semantic_key, raw) in rows {
            debug_assert!(context.permitted.contains(&assertion_id));
            debug_assert!(context.entities.contains_key(&from));
            debug_assert!(context.entities.contains_key(&to));
            debug_assert!(!seen.contains(&assertion_id));
            let value: serde_json::Value =
                serde_json::from_str(&raw).map_err(|_| "world-projection-corrupt")?;
            let payload = decode_versioned("world_relation", &value)
                .map_err(|_| "world-projection-corrupt".to_string())?;
            let WorldView::Relation(view) = payload.view(semantic_key.as_deref().unwrap_or(""))
            else {
                return Err("world-projection-corrupt".into());
            };
            let relation = view.payload;
            debug_assert!(flags_allow(input.flags, relation.relation_type));
            seen.insert(assertion_id.clone());
            let evaluation = evaluate_conditions(
                &assertion_id,
                &relation.conditions,
                &context.observations.conditions,
            );
            let condition_state = evaluation.aggregate;
            load.edges.push(WorldEdgeV2 {
                assertion_id,
                semantic_key: semantic_key.unwrap_or_default(),
                from: from.clone(),
                to: to.clone(),
                relation_type: relation.relation_type,
                lifecycle: if status == "disputed" {
                    saaa_personal_state_core::Status::Disputed
                } else {
                    saaa_personal_state_core::Status::Active
                },
                epistemic: relation.epistemic,
                basis: basis_name(relation.basis),
                comparison_id: relation.comparison_id,
                target_direction: relation.target_direction,
                correlation_sign: relation.correlation_sign,
                confidence: relation.confidence,
                strength: relation.strength,
                evidence: relation
                    .evidence_stances
                    .iter()
                    .map(|stance| SliceEvidenceV2 {
                        source: stance.source.clone(),
                        stance: stance.stance,
                    })
                    .collect(),
                conditions: relation.conditions,
                condition_state,
                mechanism: relation.mechanism,
                effect_input: relation.effect_input,
            });
            for endpoint in [from, to] {
                if !expanded.contains(&endpoint) && !next_frontier.contains(&endpoint) {
                    next_frontier.push(endpoint);
                }
            }
        }
        next_frontier.sort();
        frontier = next_frontier;
    }
    load.edges
        .sort_by(|a, b| a.assertion_id.cmp(&b.assertion_id));
    Ok(load)
}
pub(super) fn basis_name(basis: saaa_personal_state_core::world::Basis) -> String {
    match basis {
        saaa_personal_state_core::world::Basis::UserStatement => "user_statement".into(),
        saaa_personal_state_core::world::Basis::ModelHypothesis => "model_hypothesis".into(),
    }
}
pub(super) fn relation_to_slice(edge: &WorldEdgeV2, observations: &ValidatedObservations) -> SliceRelationV2 {
    let evaluation = evaluate_conditions(
        &edge.assertion_id,
        &edge.conditions,
        &observations.conditions,
    );
    SliceRelationV2 {
        assertion_id: edge.assertion_id.clone(),
        semantic_key: edge.semantic_key.clone(),
        from_entity_id: edge.from.clone(),
        to_entity_id: edge.to.clone(),
        relation_type: edge.relation_type,
        lifecycle: edge.lifecycle,
        epistemic: edge.epistemic,
        basis: edge.basis.clone(),
        target_direction: edge.target_direction,
        correlation_sign: edge.correlation_sign,
        strength: edge.strength.clone(),
        confidence: edge.confidence.clone(),
        comparison_id: edge.comparison_id.clone(),
        condition_results: evaluation
            .items
            .into_iter()
            .map(|item| ConditionResultV2 {
                relation_assertion_id: item.relation_assertion_id,
                key: item.key,
                expected_value: item.expected_value,
                state: item.state,
                evidence: item.evidence,
            })
            .collect(),
        condition_state: evaluation.aggregate,
        evidence: edge.evidence.clone(),
    }
}
pub(super) fn seed_text(seed: &WorldSeed) -> String {
    match seed {
        WorldSeed::ExactName(name) => saaa_personal_state_core::world::normalize_name(name),
        WorldSeed::EntityId(id) => id.clone(),
    }
}
pub(super) fn slice_focus_of(candidate: &FocusCandidate) -> SliceFocusV2 {
    SliceFocusV2 {
        entity_id: candidate.entity_id.clone(),
        reason: candidate.reason.clone(),
        objective_assertion_id: candidate.objective_assertion_id.clone(),
    }
}
pub(super) fn goal_ids(nodes: &[SliceNodeV2]) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.entity_kind == EntityKindV2::Goal)
        .map(|node| node.entity_id.clone())
        .collect()
}
pub(super) fn project_ids(nodes: &[SliceNodeV2]) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.entity_kind == EntityKindV2::Project)
        .map(|node| node.entity_id.clone())
        .collect()
}
pub(super) fn node_of(entity: &EntityV2Row) -> SliceNodeV2 {
    SliceNodeV2 {
        entity_id: entity.entity_id.clone(),
        entity_kind: entity.kind,
        name: entity.name.clone(),
        assertion_id: entity.assertion_id.clone(),
        objective_assertion_id: entity.objective_assertion_id.clone(),
    }
}
pub(super) struct PathView {
    pub(super) nodes: Vec<SliceNodeV2>,
    pub(super) relations: Vec<SliceRelationV2>,
    pub(super) relevance: Option<RelevancePathV2>,
    pub(super) causal: Option<CausalPathV2>,
}
pub(super) fn path_view(
    path: &WorldPathV2,
    edges: &[WorldEdgeV2],
    context: &QueryContextV2,
    focus_entity_id: Option<&str>,
    goal_id: Option<String>,
    evaluate_reverse: bool,
) -> Option<PathView> {
    let mut nodes = Vec::new();
    for id in &path.nodes {
        nodes.push(node_of(context.entities.get(id)?));
    }
    let mut relations = Vec::new();
    let mut steps = Vec::new();
    for step in &path.steps {
        let edge = edges.get(step.edge)?;
        relations.push(relation_to_slice(edge, &context.observations));
        steps.push(CausalStepV2 {
            assertion_id: edge.assertion_id.clone(),
            traversed_reverse: step.traversed_reverse,
        });
    }
    // Direction is always composed in the edges' declared from->to order. A
    // reverse search walks the path backwards, so reverse the evaluation order.
    let mut edge_refs: Vec<&WorldEdgeV2> = path.steps.iter().map(|s| &edges[s.edge]).collect();
    if evaluate_reverse {
        edge_refs.reverse();
    }
    let evaluation = evaluate_path(&edge_refs);
    let relevance = focus_entity_id.map(|focus| RelevancePathV2 {
        node_ids: path.nodes.clone(),
        steps: steps.clone(),
        focus_entity_id: focus.to_string(),
        goal_id,
        condition_state: path.condition_state,
        truncated: path.truncated,
    });
    let causal = Some(CausalPathV2 {
        node_ids: path.nodes.clone(),
        steps,
        hops: path.steps.len(),
        derived: true,
        direction: evaluation.direction,
        confidence: evaluation.confidence,
        condition_state: path.condition_state,
        truncated: path.truncated,
    });
    Some(PathView {
        nodes,
        relations,
        relevance,
        causal,
    })
}
