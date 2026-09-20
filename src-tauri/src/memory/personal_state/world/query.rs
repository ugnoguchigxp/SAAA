//! Bounded, side-effect-free World query (WM-09..WM-13).
//!
//! The reader never writes or repairs the projection. A stale or missing
//! projection is reported as an omission notice, not an error; authorization
//! failures are contract errors, never an empty slice.
//!
//! Every projected row is re-authorized and re-checked for current validity at
//! read time (R1/R2): the projection is a bounded index, not an authority.

use crate::database_error;
use rusqlite::{named_params, Connection};
use saaa_personal_state_core::world::model::{EntityKind, SliceEvidence, SliceFocus, SliceNode};
use saaa_personal_state_core::world::relevance::{
    build_gaps, focus_rank, node, relation as slice_relation, slice_path, trim_to_budget, GapInput,
    MAX_SLICE_BYTES,
};
use saaa_personal_state_core::world::traversal::{
    conditions_consistent, maximal_only, traverse, CausalDirection, EdgeStatus, Limits,
    TraversalMode, WorldEdge, WorldPath,
};
use saaa_personal_state_core::world::{normalize_name, WorldPayload, WorldSlice};
use saaa_personal_state_core::{AccessRequest, Classification, Purpose};
use std::collections::{BTreeMap, BTreeSet};

pub const STALE: &str = "projection_stale";
pub const PENDING: &str = "pending_review";
pub const UNKNOWN_SEED: &str = "unknown_seed";
pub const AMBIGUOUS_SEED: &str = "ambiguous_seed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldSeed {
    EntityId(String),
    ExactName(String),
}

pub struct ActivateInput<'a> {
    pub project_scope: &'a str,
    pub access: &'a AccessRequest<'a>,
    pub now: i64,
    pub seeds: &'a [WorldSeed],
    pub causal_direction: CausalDirection,
    pub limits: Limits,
    /// Requested upper bound; only the maximum is capped. A budget too small to
    /// encode a non-empty slice is `world-budget-too-small`, never enlarged (R8).
    pub max_bytes: usize,
}

struct ScopeState {
    revision: u64,
    input_epoch: u64,
    policy_revision: u64,
    principal: String,
}

/// Assertion-level access re-check (R1). Resolved once per query into an id set;
/// every projected read is filtered by that set, so a row whose stored
/// AccessScope does not permit this request never reaches names, paths or Focus.
const PERMITTED_ASSERTION: &str = "
      a.erased=0
  AND json_extract(a.metadata,'$.kind') IN ('world_entity','world_relation','world_focus')
  AND json_extract(a.metadata,'$.access.principal')=:principal
  AND json_extract(a.metadata,'$.access.scope')=:scope
  AND (json_extract(a.metadata,'$.access.task_request') IS NULL
       OR json_extract(a.metadata,'$.access.task_request')=:task_request)
  AND json_extract(a.metadata,'$.access.policy_revision')=:policy
  AND CASE json_extract(a.metadata,'$.access.classification')
        WHEN 'public' THEN 0 WHEN 'internal' THEN 1
        WHEN 'confidential' THEN 2 WHEN 'restricted' THEN 3 ELSE 4 END <= :classification
  AND EXISTS(SELECT 1 FROM json_each(a.metadata,'$.access.purposes') WHERE value=:purpose)";

fn classification_level(value: Classification) -> i64 {
    match value {
        Classification::Public => 0,
        Classification::Internal => 1,
        Classification::Confidential => 2,
        Classification::Restricted => 3,
    }
}

fn purpose_name(value: Purpose) -> &'static str {
    match value {
        Purpose::Tactical => "tactical",
        Purpose::Reasoning => "reasoning",
        Purpose::StateExtract => "state_extract",
        Purpose::Diagnostics => "diagnostics",
    }
}

pub fn activate(c: &Connection, input: &ActivateInput<'_>) -> Result<WorldSlice, String> {
    let access = input.access;
    if !access.authorized
        || access.scope != "primary"
        || access.task_request != Some(input.project_scope)
        || !matches!(
            access.purpose,
            Purpose::Reasoning | Purpose::StateExtract | Purpose::Diagnostics
        )
        || access.max_classification < Classification::Internal
    {
        return Err("world-scope-denied".into());
    }
    if input.project_scope.is_empty() || !input.project_scope.starts_with("project:") {
        return Err("world-scope-denied".into());
    }
    // At most four seeds; more would let a caller amplify resolution work.
    if input.seeds.is_empty() || input.seeds.len() > 4 {
        return Err("world-limit".into());
    }
    // Fixed exploration maxima: a caller may ask for less, never more.
    let limits = input.limits.capped();
    let scope = scope_state(c)?;
    if access.policy_revision != scope.policy_revision || access.principal != scope.principal {
        return Err("world-scope-denied".into());
    }
    if !project_active(c, input.project_scope)? {
        return Err("world-scope-denied".into());
    }
    // R8: only cap the maximum. A small request reaches trim_to_budget and is
    // rejected explicitly instead of being silently enlarged to 256 bytes.
    let max_bytes = input.max_bytes.min(MAX_SLICE_BYTES);
    let mut slice = WorldSlice::empty(scope.revision, input.now);

    // A newer unprocessed source in this project is reported before any global
    // epoch staleness, so it is never confused with a rebuild wait.
    if pending_review(c, input.project_scope)? {
        slice.notices.push(PENDING.into());
        return trim_to_budget(slice, max_bytes).map_err(|e| e.code().to_string());
    }

    // Projection integrity: revision + epoch + policy against the same read.
    // A v2 projection is intentionally unreadable through the v1 API (D20).
    let meta: Option<(u64, u64, u64, i64)> = c
        .query_row(
            "SELECT ledger_revision,input_epoch,policy_revision,projection_version
               FROM personal_world_projection_meta WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();
    let Some((revision, epoch, policy, projection_version)) = meta else {
        slice.notices.push(STALE.into());
        return trim_to_budget(slice, max_bytes).map_err(|e| e.code().to_string());
    };
    if projection_version != 1
        || revision != scope.revision
        || epoch != scope.input_epoch
        || policy != scope.policy_revision
    {
        slice.notices.push(STALE.into());
        return trim_to_budget(slice, max_bytes).map_err(|e| e.code().to_string());
    }

    let permitted = permitted_assertions(c, access, input.project_scope)?;
    let (entities, aliases) = load_entities(c, input.project_scope, input.now, &permitted)?;
    let (seeds, notice) = resolve_seeds(input, &entities, &aliases);
    if let Some(notice) = notice {
        slice.notices.push(notice.into());
        return trim_to_budget(slice, max_bytes).map_err(|e| e.code().to_string());
    }
    if seeds.is_empty() {
        slice.notices.push(UNKNOWN_SEED.into());
        return trim_to_budget(slice, max_bytes).map_err(|e| e.code().to_string());
    }

    // R3: the SQL fetch is bounded by `scan`, but it does not consume the
    // traversal scan budget. Fetching and expansion are counted separately so a
    // project with more than `scan` relations can still traverse from the seed.
    let (edges, more) = load_edges(
        c,
        input.project_scope,
        input.now,
        &permitted,
        &entities,
        &seeds,
        limits.scan,
    )?;
    let effective = limits;

    let related = traverse(
        &edges,
        &seeds,
        effective,
        TraversalMode::Related,
        CausalDirection::Forward,
    );
    let causal = traverse(
        &edges,
        &seeds,
        effective,
        TraversalMode::Causal,
        input.causal_direction,
    );

    // Bound Focus selection by the same node budget as the slice (R7). A
    // truncated Focus fetch is reported so it is never mistaken for "no focus".
    let (focus, focus_more) =
        load_focus(c, input.project_scope, input.now, &permitted, limits.nodes)?;
    let focus_ids: BTreeSet<String> = focus.iter().map(|f| f.entity_id.clone()).collect();

    // Relevance paths only when they reach a Focus entity.
    let mut relevance: Vec<WorldPath> = related
        .paths
        .iter()
        .filter(|p| p.nodes.last().is_some_and(|n| focus_ids.contains(n)))
        .cloned()
        .collect();
    // R5: drop condition-inconsistent chains *before* maximal-path selection so a
    // longer inapplicable chain cannot erase a valid shorter prefix.
    let consistent: Vec<WorldPath> = causal
        .paths
        .iter()
        .filter(|path| {
            let path_edges: Vec<&WorldEdge> = path.steps.iter().map(|s| &edges[s.edge]).collect();
            conditions_consistent(&path_edges)
        })
        .cloned()
        .collect();
    let mut causal_paths = maximal_only(&consistent);

    // R10: the return cap applies to usable paths, not intermediate prefixes.
    let mut paths_truncated = false;
    if relevance.len() > limits.paths {
        relevance.truncate(limits.paths);
        paths_truncated = true;
    }
    if causal_paths.len() > limits.paths {
        causal_paths.truncate(limits.paths);
        paths_truncated = true;
    }

    let (nodes, present) = collect_nodes(
        &entities,
        &relevance,
        &causal_paths,
        &focus,
        effective.nodes,
    );
    let relevance: Vec<WorldPath> = relevance
        .into_iter()
        .filter(|p| p.nodes.iter().all(|n| present.contains(n)))
        .collect();
    let causal_paths: Vec<WorldPath> = causal_paths
        .into_iter()
        .filter(|p| p.nodes.iter().all(|n| present.contains(n)))
        .collect();
    let focus: Vec<FocusRow> = focus
        .into_iter()
        .filter(|f| present.contains(&f.entity_id))
        .collect();

    slice.nodes = nodes;
    let used_edges: BTreeSet<usize> = relevance
        .iter()
        .chain(causal_paths.iter())
        .flat_map(|p| p.steps.iter().map(|s| s.edge))
        .collect();
    slice.relations = used_edges
        .iter()
        .map(|index| enrich_relation(c, &edges[*index]))
        .collect::<Result<Vec<_>, String>>()?;
    slice.relevance_paths = relevance.iter().map(|p| slice_path(p, &edges)).collect();
    slice.causal_paths = causal_paths.iter().map(|p| slice_path(p, &edges)).collect();
    slice.focus = focus
        .iter()
        .map(|f| SliceFocus {
            entity_id: f.entity_id.clone(),
            reason: f.reason.clone(),
            objective_assertion_id: f.objective_assertion_id.clone(),
        })
        .collect();
    slice.truncated =
        related.truncated || causal.truncated || more || paths_truncated || focus_more;
    if let Some(reason) =
        related
            .truncation_reason
            .or(causal.truncation_reason)
            .or(if paths_truncated {
                Some("paths")
            } else if more {
                Some("scan")
            } else if focus_more {
                Some("nodes")
            } else {
                None
            })
    {
        slice.notices.push(format!("truncated:{reason}"));
    }

    // R6: gaps come from the exact (Focus, causal edge) pairs on relevance paths,
    // never a cross product of unrelated Focus. Ordering also uses the Focus区分
    // and the path length as the plan fixes.
    let focus_ranks: BTreeMap<String, u8> = focus
        .iter()
        .map(|f| (f.entity_id.clone(), focus_rank(&f.reason)))
        .collect();
    let mut relevant_causal: Vec<GapInput<'_>> = Vec::new();
    for path in &relevance {
        let Some(focus_id) = path.nodes.last() else {
            continue;
        };
        let Some(rank) = focus_ranks.get(focus_id).copied() else {
            continue;
        };
        for step in &path.steps {
            let edge = &edges[step.edge];
            if edge.is_causal() {
                relevant_causal.push(GapInput {
                    focus_entity_id: focus_id.clone(),
                    focus_rank: rank,
                    path_len: path.steps.len(),
                    edge,
                });
            }
        }
    }
    let focus_names: BTreeMap<String, String> = slice
        .nodes
        .iter()
        .map(|n| (n.entity_id.clone(), n.name.clone()))
        .collect();
    slice.research_gaps = build_gaps(input.project_scope, &focus_names, &relevant_causal);

    trim_to_budget(slice, max_bytes).map_err(|e| e.code().to_string())
}

fn scope_state(c: &Connection) -> Result<ScopeState, String> {
    c.query_row(
        "SELECT revision,input_epoch,policy_revision,principal FROM personal_scope WHERE id='primary'",
        [],
        |r| {
            Ok(ScopeState {
                revision: r.get(0)?,
                input_epoch: r.get(1)?,
                policy_revision: r.get(2)?,
                principal: r.get(3)?,
            })
        },
    )
    .map_err(|_| "world-projection-corrupt".to_string())
}

/// The project scope must still be active; a revoked scope stops all World reads (R1).
fn project_active(c: &Connection, project_scope: &str) -> Result<bool, String> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM context_scopes WHERE scope_key=?1 AND state='active')",
        [project_scope],
        |r| r.get(0),
    )
    .map_err(database_error)
}

/// Non-erased World assertions whose stored AccessScope permits this request (R1).
fn permitted_assertions(
    c: &Connection,
    access: &AccessRequest<'_>,
    project_scope: &str,
) -> Result<BTreeSet<String>, String> {
    let sql = format!(
        "SELECT a.id FROM personal_assertions a
          WHERE {PERMITTED_ASSERTION}"
    );
    let mut statement = c.prepare(&sql).map_err(database_error)?;
    let rows = statement
        .query_map(
            named_params! {
                ":principal": access.principal,
                ":scope": access.scope,
                ":task_request": project_scope,
                ":policy": access.policy_revision as i64,
                ":classification": classification_level(access.max_classification),
                ":purpose": purpose_name(access.purpose),
            },
            |r| r.get::<_, String>(0),
        )
        .map_err(database_error)?;
    rows.collect::<Result<BTreeSet<_>, _>>()
        .map_err(database_error)
}

fn pending_review(c: &Connection, project_scope: &str) -> Result<bool, String> {
    c.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM personal_sources p
           JOIN personal_jobs j ON j.source_sequence=p.sequence
           WHERE p.available=1 AND p.bytes>0 AND j.status!='completed'
             AND EXISTS(SELECT 1 FROM personal_source_scope_refs r
                        WHERE r.source_id=p.message_id AND r.version=p.version
                          AND r.scope_key=?1)
             AND p.sequence > COALESCE((
               SELECT MAX(ps.sequence) FROM personal_dependencies d
               JOIN personal_assertions a ON a.id=d.assertion_id
               JOIN personal_sources ps ON ps.message_id=d.dependency_id
               WHERE d.dependency_kind='source' AND a.erased=0
                 AND json_extract(a.metadata,'$.kind') IN ('world_entity','world_relation','world_focus')
                 AND json_extract(a.metadata,'$.access.task_request')=?1
             ),0)
         )",
        [project_scope],
        |r| r.get(0),
    )
    .map_err(database_error)
}

struct EntityRow {
    name: String,
    kind: String,
    canonical_name: String,
}

type EntityIndex = (
    BTreeMap<String, EntityRow>,
    BTreeMap<String, BTreeSet<String>>,
);

/// Loads only permitted, currently-active entities and their aliases. A time-only
/// expiry therefore removes an entity, its aliases and every edge that touches it (R2).
fn load_entities(
    c: &Connection,
    project_scope: &str,
    now: i64,
    permitted: &BTreeSet<String>,
) -> Result<EntityIndex, String> {
    let mut entities: BTreeMap<String, EntityRow> = BTreeMap::new();
    let mut statement = c
        .prepare(
            "SELECT assertion_id,entity_id,name,kind,canonical_name,status,valid_from,valid_until
               FROM personal_world_entities WHERE project_scope=:project",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(named_params! { ":project": project_scope }, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, Option<i64>>(7)?,
            ))
        })
        .map_err(database_error)?;
    for row in rows {
        let (assertion_id, entity_id, name, kind, canonical_name, status, valid_from, valid_until) =
            row.map_err(database_error)?;
        if !permitted.contains(&assertion_id)
            || status != "active"
            || valid_from > now
            || valid_until.is_some_and(|until| now >= until)
        {
            continue;
        }
        entities.insert(
            entity_id,
            EntityRow {
                name,
                kind,
                canonical_name,
            },
        );
    }

    let mut aliases: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut statement = c
        .prepare(
            "SELECT assertion_id,entity_id,canonical_alias
               FROM personal_world_aliases WHERE project_scope=:project",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(named_params! { ":project": project_scope }, |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })
        .map_err(database_error)?;
    for row in rows {
        let (assertion_id, entity_id, canonical_alias) = row.map_err(database_error)?;
        if !permitted.contains(&assertion_id) || !entities.contains_key(&entity_id) {
            continue;
        }
        aliases
            .entry(canonical_alias)
            .or_default()
            .insert(entity_id);
    }
    Ok((entities, aliases))
}

/// Resolve seeds ID-first, then canonical name, then alias, against the already
/// authorized and currently-valid entity set. Every requested seed must resolve
/// to exactly one entity; a missing or ambiguous seed is reported and the whole
/// query is omitted rather than silently dropping that seed.
fn resolve_seeds(
    input: &ActivateInput<'_>,
    entities: &BTreeMap<String, EntityRow>,
    aliases: &BTreeMap<String, BTreeSet<String>>,
) -> (Vec<String>, Option<&'static str>) {
    let mut resolved: BTreeSet<String> = BTreeSet::new();
    for seed in input.seeds {
        let matches: BTreeSet<String> = match seed {
            WorldSeed::EntityId(id) => entities
                .contains_key(id)
                .then(|| id.clone())
                .into_iter()
                .collect(),
            WorldSeed::ExactName(name) => {
                let canonical = normalize_name(name);
                let mut matches: BTreeSet<String> = entities
                    .iter()
                    .filter(|(_, entity)| entity.canonical_name == canonical)
                    .map(|(id, _)| id.clone())
                    .collect();
                if let Some(ids) = aliases.get(&canonical) {
                    matches.extend(ids.iter().cloned());
                }
                matches
            }
        };
        match matches.len() {
            0 => return (Vec::new(), Some(UNKNOWN_SEED)),
            1 => resolved.extend(matches),
            _ => return (Vec::new(), Some(AMBIGUOUS_SEED)),
        }
    }
    if resolved.is_empty() {
        return (Vec::new(), Some(UNKNOWN_SEED));
    }
    (resolved.into_iter().collect(), None)
}

fn load_edges(
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

fn relation_semantic_key(c: &Connection, assertion_id: &str) -> Result<String, String> {
    c.query_row(
        "SELECT json_extract(metadata,'$.semantic_key') FROM personal_assertions WHERE id=?1",
        [assertion_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .map_err(database_error)
    .map(|value| value.unwrap_or_default())
}

fn load_relation_payload(
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

fn enrich_relation(
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

struct FocusRow {
    entity_id: String,
    reason: String,
    objective_assertion_id: Option<String>,
}

/// Loads permitted, currently-active Focus. A `current_work` Focus is also
/// dropped as soon as its Objective expires or is erased (R2). Returns whether
/// the node-budget fetch was truncated.
fn load_focus(
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
fn collect_nodes(
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
fn parse_kind(kind: &str) -> Option<EntityKind> {
    match kind {
        "project" => Some(EntityKind::Project),
        "concept" => Some(EntityKind::Concept),
        "metric" => Some(EntityKind::Metric),
        _ => None,
    }
}
