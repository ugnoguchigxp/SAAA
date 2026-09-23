use super::*;
pub(super) fn load_edges(
    c: &Connection,
    project_scope: &str,
    now: i64,
    permitted: &BTreeSet<String>,
    entities: &BTreeMap<String, EntityRow>,
    seeds: &[String],
    scan: usize,
) -> Result<(Vec<WorldEdge>, bool), String> {
    let seed_json = serde_json::to_string(seeds).map_err(|_| "world-projection-corrupt")?;
    let limit = scan.saturating_add(1).max(1);
    let mut statement = c
        .prepare(
            "SELECT assertion_id,from_entity_id,to_entity_id,relation_type,status
               FROM personal_world_relations
              WHERE project_scope=:project AND valid_from<=:now
                AND (valid_until IS NULL OR :now<valid_until)
              ORDER BY CASE WHEN EXISTS(
                         SELECT 1 FROM json_each(:seeds)
                         WHERE value IN (from_entity_id,to_entity_id)
                       ) THEN 0 ELSE 1 END,
                       relation_type,from_entity_id,to_entity_id,assertion_id
              LIMIT :limit",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(
            named_params! {
                ":project": project_scope,
                ":now": now,
                ":seeds": seed_json,
                ":limit": limit as i64,
            },
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let more = rows.len() > scan;
    let mut edges = Vec::new();
    for (assertion_id, from, to, relation_type, status) in rows.into_iter().take(scan) {
        if !permitted.contains(&assertion_id)
            || !entities.contains_key(&from)
            || !entities.contains_key(&to)
        {
            continue;
        }
        let payload = load_relation_payload(c, &assertion_id)?;
        let Some(WorldPayload::Relation(relation)) = payload else {
            return Err("world-projection-corrupt".into());
        };
        edges.push(WorldEdge {
            semantic_key: relation_semantic_key(c, &assertion_id)?,
            assertion_id,
            from,
            to,
            relation_type,
            status: if status == "disputed" {
                EdgeStatus::Disputed
            } else {
                EdgeStatus::Active
            },
            conditions: relation
                .conditions
                .iter()
                .map(|c| (c.key.clone(), c.value.clone()))
                .collect(),
            basis: match relation.basis {
                saaa_personal_state_core::world::Basis::UserStatement => "user_statement".into(),
                saaa_personal_state_core::world::Basis::ModelHypothesis => {
                    "model_hypothesis".into()
                }
            },
        });
    }
    Ok((edges, more))
}
pub(super) fn relation_semantic_key(c: &Connection, assertion_id: &str) -> Result<String, String> {
    c.query_row(
        "SELECT json_extract(metadata,'$.semantic_key') FROM personal_assertions WHERE id=?1",
        [assertion_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .map_err(database_error)
    .map(|value| value.unwrap_or_default())
}
pub(super) fn load_relation_payload(
    c: &Connection,
    assertion_id: &str,
) -> Result<Option<WorldPayload>, String> {
    let raw: Option<String> = c
        .query_row(
            "SELECT p.value_json FROM personal_assertions a
               JOIN personal_payloads p ON p.id=a.payload_id WHERE a.id=?1",
            [assertion_id],
            |r| r.get(0),
        )
        .ok();
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| "world-projection-corrupt")?;
    WorldPayload::decode("world_relation", &value)
        .map(Some)
        .map_err(|_| "world-projection-corrupt".into())
}
pub(super) fn enrich_relation(
    c: &Connection,
    edge: &WorldEdge,
) -> Result<saaa_personal_state_core::world::SliceRelation, String> {
    let mut relation = slice_relation(edge);
    if let Some(WorldPayload::Relation(payload)) = load_relation_payload(c, &edge.assertion_id)? {
        relation.evidence = payload
            .evidence_stances
            .iter()
            .map(|stance| SliceEvidence {
                source_id: stance.source.id.clone(),
                version: stance.source.version,
                stance: match stance.stance {
                    saaa_personal_state_core::world::Stance::Supports => "supports",
                    saaa_personal_state_core::world::Stance::Challenges => "challenges",
                    saaa_personal_state_core::world::Stance::Context => "context",
                }
                .to_string(),
            })
            .collect();
    }
    Ok(relation)
}
pub(super) struct FocusRow {
    pub(super) entity_id: String,
    pub(super) reason: String,
    pub(super) objective_assertion_id: Option<String>,
}
/// Loads permitted, currently-active Focus. A `current_work` Focus is also
/// dropped as soon as its Objective expires or is erased (R2). Returns whether
/// the node-budget fetch was truncated.
pub(super) fn load_focus(
    c: &Connection,
    project_scope: &str,
    now: i64,
    permitted: &BTreeSet<String>,
    limit: usize,
) -> Result<(Vec<FocusRow>, bool), String> {
    let fetch = limit.max(1) + 1;
    let mut statement = c
        .prepare(
            "SELECT f.assertion_id,f.entity_id,f.reason,f.objective_assertion_id,
                    f.valid_from,f.valid_until,
                    oa.erased, json_extract(oa.metadata,'$.valid_until')
               FROM personal_world_focus f
               LEFT JOIN personal_assertions oa ON oa.id=f.objective_assertion_id
              WHERE f.project_scope=:project AND f.status='active'
                AND f.valid_from<=:now AND (f.valid_until IS NULL OR :now<f.valid_until)
              ORDER BY CASE f.reason WHEN 'current_work' THEN 0 ELSE 1 END, f.entity_id
              LIMIT :limit",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(
            named_params! { ":project": project_scope, ":now": now, ":limit": fetch as i64 },
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<bool>>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                ))
            },
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let more = rows.len() > limit.max(1);
    let mut focus = Vec::new();
    for (
        assertion_id,
        entity_id,
        reason,
        objective_assertion_id,
        objective_erased,
        objective_until,
    ) in rows.into_iter().take(limit.max(1))
    {
        if !permitted.contains(&assertion_id) {
            continue;
        }
        if reason == "current_work"
            && (objective_assertion_id.is_none()
                || objective_erased.unwrap_or(true)
                || objective_until.is_some_and(|until| now >= until))
        {
            continue;
        }
        focus.push(FocusRow {
            entity_id,
            reason,
            objective_assertion_id,
        });
    }
    focus.sort_by(|a, b| {
        (focus_rank(&a.reason), &a.entity_id).cmp(&(focus_rank(&b.reason), &b.entity_id))
    });
    Ok((focus, more))
}
/// Selects nodes for the slice within the node budget, adding whole paths only.
/// Returns the present ids so callers can drop any path/Focus that did not fit (R7).
pub(super) fn collect_nodes(
    entities: &BTreeMap<String, EntityRow>,
    relevance: &[WorldPath],
    causal: &[WorldPath],
    focus: &[FocusRow],
    max_nodes: usize,
) -> (Vec<SliceNode>, BTreeSet<String>) {
    let mut ids: Vec<String> = Vec::new();
    let mut present: BTreeSet<String> = BTreeSet::new();
    for path in relevance.iter().chain(causal.iter()) {
        if !path.nodes.iter().all(|n| entities.contains_key(n)) {
            continue;
        }
        let missing: Vec<&String> = path
            .nodes
            .iter()
            .filter(|n| !present.contains(*n))
            .collect();
        if present.len() + missing.len() > max_nodes {
            continue;
        }
        for id in missing {
            present.insert(id.clone());
            ids.push(id.clone());
        }
    }
    for row in focus {
        if present.len() >= max_nodes {
            break;
        }
        if entities.contains_key(&row.entity_id) && present.insert(row.entity_id.clone()) {
            ids.push(row.entity_id.clone());
        }
    }
    let nodes = ids
        .iter()
        .filter_map(|id| {
            let entity = entities.get(id)?;
            Some(node(id, &entity.name, parse_kind(&entity.kind)?))
        })
        .collect();
    (nodes, present)
}
/// Only the three v1 kinds are understood by the v1 reader. A v2 `goal`/`actor`
/// row is never silently downgraded to `concept` (D20).
pub(super) fn parse_kind(kind: &str) -> Option<EntityKind> {
    match kind {
        "project" => Some(EntityKind::Project),
        "concept" => Some(EntityKind::Concept),
        "metric" => Some(EntityKind::Metric),
        _ => None,
    }
}
