//! Internal v2 World query (D21/D23/D31/D32/C7/C8).
//!
//! Read-only. A stale or missing projection is an omission notice, never a
//! repair. Authorization failures are contract errors. Every projected row is
//! re-authorized and re-checked for current validity at read time.

#![allow(clippy::too_many_arguments)]

use super::observations_v2::{
    validate_observations, AvailabilityObservationInput, ConditionObservationInput,
    ValidatedObservations,
};
use super::query::{WorldSeed, AMBIGUOUS_SEED, PENDING, STALE, UNKNOWN_SEED};
use crate::database_error;
use crate::memory::personal_state::store;
use rusqlite::{named_params, Connection};
use saaa_personal_state_core::world::conditions_v2::{evaluate_availability, evaluate_conditions};
use saaa_personal_state_core::world::model_v2::{EffectDirection, EntityKindV2, RelationTypeV2};
use saaa_personal_state_core::world::relevance_v2::{
    build_gap_candidates, dependencies, focus_rank_v2, order_focus, temporary_attention_focus,
    FocusCandidate, GapInputV2, GapRelationV2, OutcomeConflictV2, UnknownAvailabilityV2,
};
use saaa_personal_state_core::world::slice_v2::*;
use saaa_personal_state_core::world::traversal_v2::{
    evaluate_path, maximal_only_v2, traverse_v2, CausalDirection, LimitsV2, TraversalModeV2,
    WorldEdgeV2, WorldPathV2,
};
use saaa_personal_state_core::world::versioned::{decode_versioned, WorldView};
use saaa_personal_state_core::{AccessRequest, Classification, Ledger, Purpose};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncludeFlags {
    pub causal: bool,
    pub goals: bool,
    pub correlations: bool,
    pub dependencies: bool,
}

impl Default for IncludeFlags {
    fn default() -> Self {
        Self {
            causal: true,
            goals: true,
            correlations: true,
            dependencies: true,
        }
    }
}

pub struct ActivateInputV2<'a> {
    pub project_scope: &'a str,
    pub access: &'a AccessRequest<'a>,
    pub now: i64,
    pub seeds: &'a [WorldSeed],
    pub causal_direction: CausalDirection,
    pub limits: LimitsV2,
    pub max_bytes: usize,
    pub request_id: &'a str,
    pub explicit_question: bool,
    pub flags: IncludeFlags,
    pub condition_observations: &'a [ConditionObservationInput],
    pub availability_observations: &'a [AvailabilityObservationInput],
    pub temporary_attention_entity_ids: &'a [String],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryStatsV2 {
    pub fetch_rows: usize,
    pub scan_steps: usize,
}

#[derive(Debug, Clone)]
pub struct EntityV2Row {
    pub assertion_id: String,
    pub entity_id: String,
    pub kind: EntityKindV2,
    pub name: String,
    pub objective_assertion_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FocusRowV2 {
    pub assertion_id: String,
    pub entity_id: String,
    pub reason: String,
    pub objective_assertion_id: Option<String>,
}

#[derive(Debug, Default)]
pub struct QueryContextV2 {
    pub revision: u64,
    pub as_of_ms: i64,
    pub permitted: BTreeSet<String>,
    pub entities: BTreeMap<String, EntityV2Row>,
    pub aliases: BTreeMap<String, BTreeSet<String>>,
    pub focus: Vec<FocusRowV2>,
    pub observations: ValidatedObservations,
    pub notices: Vec<String>,
    pub seeds: Vec<String>,
    pub stale: bool,
    pub pending: bool,
}

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

fn authorize_input(input: &ActivateInputV2<'_>) -> Result<(), String> {
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
    if input.seeds.is_empty() {
        return Err("world-limit".into());
    }
    // Duplicate seeds are removed after normalization before the 4-seed cap.
    let unique_seeds: BTreeSet<String> = input
        .seeds
        .iter()
        .map(|seed| match seed {
            WorldSeed::EntityId(id) => format!("id:{id}"),
            WorldSeed::ExactName(name) => {
                format!(
                    "name:{}",
                    saaa_personal_state_core::world::normalize_name(name)
                )
            }
        })
        .collect();
    if unique_seeds.len() > 4 {
        return Err("world-limit".into());
    }
    if input.temporary_attention_entity_ids.len() > 4 {
        return Err("world-limit".into());
    }
    Ok(())
}

struct ScopeState {
    revision: u64,
    input_epoch: u64,
    policy_revision: u64,
    principal: String,
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

fn project_active(c: &Connection, project_scope: &str) -> Result<bool, String> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM context_scopes WHERE scope_key=?1 AND state='active')",
        [project_scope],
        |r| r.get(0),
    )
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

const PERMITTED_ASSERTION: &str = "
      a.erased=0
  AND json_extract(a.metadata,'$.kind') IN ('world_entity','world_relation','world_focus')
  AND json_extract(a.metadata,'$.access.principal')=:principal
  AND json_extract(a.metadata,'$.access.scope')=:scope
  AND (json_extract(a.metadata,'$.access.task_request') IS NULL
       OR json_extract(a.metadata,'$.access.task_request')=:task_request)
  AND json_extract(a.metadata,'$.access.policy_revision')=:policy
  AND NOT EXISTS(
        SELECT 1 FROM json_each(a.metadata,'$.input_dependencies') dependency
        WHERE NOT EXISTS(
              SELECT 1 FROM personal_sources ps
              WHERE ps.message_id=json_extract(dependency.value,'$.id')
                AND ps.version=json_extract(dependency.value,'$.version')
                AND ps.available=1
                AND NOT EXISTS(SELECT 1 FROM personal_tombstones tombstone
                               WHERE tombstone.source_id=ps.message_id)))
  AND NOT EXISTS(
        SELECT 1 FROM personal_dependencies normalized
        WHERE normalized.assertion_id=a.id AND normalized.dependency_kind='source'
          AND NOT EXISTS(
                SELECT 1 FROM json_each(a.metadata,'$.input_dependencies') dependency
                WHERE json_extract(dependency.value,'$.id')=normalized.dependency_id))
  AND CASE json_extract(a.metadata,'$.access.classification')
        WHEN 'public' THEN 0 WHEN 'internal' THEN 1
        WHEN 'confidential' THEN 2 WHEN 'restricted' THEN 3 ELSE 4 END <= :classification
  AND EXISTS(SELECT 1 FROM json_each(a.metadata,'$.access.purposes') WHERE value=:purpose)";

fn permitted_assertions(
    c: &Connection,
    access: &AccessRequest<'_>,
    project_scope: &str,
) -> Result<BTreeSet<String>, String> {
    let sql = format!("SELECT a.id FROM personal_assertions a WHERE {PERMITTED_ASSERTION}");
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

fn kind_from_projection(kind: &str) -> Option<EntityKindV2> {
    match kind {
        "project" => Some(EntityKindV2::Project),
        "concept" => Some(EntityKindV2::Concept),
        "metric" => Some(EntityKindV2::Metric),
        "goal" => Some(EntityKindV2::Goal),
        "actor" => Some(EntityKindV2::Actor),
        _ => None,
    }
}

fn objective_for(c: &Connection, assertion_id: &str) -> Result<Option<String>, String> {
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
    match decode_versioned("world_entity", &value) {
        Ok(payload) => match payload.view("") {
            WorldView::Entity(view) => Ok(view.payload.objective_assertion_id.clone()),
            _ => Err("world-projection-corrupt".into()),
        },
        Err(_) => Err("world-projection-corrupt".into()),
    }
}

/// Load permitted, active entities. A `goal` whose Objective is no longer
/// Active is dropped here, so no expired goal name reaches the Slice (D23).
type EntityIndexV2 = (
    BTreeMap<String, EntityV2Row>,
    BTreeMap<String, BTreeSet<String>>,
);

fn load_entities(
    c: &Connection,
    ledger: &Ledger,
    project_scope: &str,
    now: i64,
    permitted: &BTreeSet<String>,
) -> Result<EntityIndexV2, String> {
    let mut entities: BTreeMap<String, EntityV2Row> = BTreeMap::new();
    let mut statement = c
        .prepare(
            "SELECT assertion_id,entity_id,name,kind,status,valid_from,valid_until
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
                r.get::<_, i64>(5)?,
                r.get::<_, Option<i64>>(6)?,
            ))
        })
        .map_err(database_error)?;
    for row in rows {
        let (assertion_id, entity_id, name, kind, status, valid_from, valid_until) =
            row.map_err(database_error)?;
        if !permitted.contains(&assertion_id)
            || status != "active"
            || valid_from > now
            || valid_until.is_some_and(|until| now >= until)
        {
            continue;
        }
        let Some(kind) = kind_from_projection(&kind) else {
            return Err("world-projection-corrupt".into());
        };
        let objective_assertion_id = if kind == EntityKindV2::Goal {
            let objective = objective_for(c, &assertion_id)?;
            match objective.as_deref() {
                // Re-check the Objective's validity, scope and access at read time.
                Some(id)
                    if ledger.status(id, now) == saaa_personal_state_core::Status::Active
                        && ledger.assertions.get(id).is_some_and(|objective| {
                            objective.access.task_request.as_deref() == Some(project_scope)
                        }) =>
                {
                    Some(id.to_string())
                }
                _ => continue,
            }
        } else {
            None
        };
        entities.insert(
            entity_id.clone(),
            EntityV2Row {
                assertion_id,
                entity_id,
                kind,
                name,
                objective_assertion_id,
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

fn resolve_seeds(
    seeds: &[WorldSeed],
    entities: &BTreeMap<String, EntityV2Row>,
    aliases: &BTreeMap<String, BTreeSet<String>>,
) -> (Vec<String>, Option<&'static str>) {
    let mut resolved: BTreeSet<String> = BTreeSet::new();
    for seed in seeds {
        let matches: BTreeSet<String> = match seed {
            WorldSeed::EntityId(id) => entities
                .contains_key(id)
                .then(|| id.clone())
                .into_iter()
                .collect(),
            WorldSeed::ExactName(name) => {
                let canonical = saaa_personal_state_core::world::normalize_name(name);
                let mut matches: BTreeSet<String> = entities
                    .iter()
                    .filter(|(_, entity)| {
                        saaa_personal_state_core::world::normalize_name(&entity.name) == canonical
                    })
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

fn load_focus(
    c: &Connection,
    ledger: &Ledger,
    project_scope: &str,
    now: i64,
    permitted: &BTreeSet<String>,
    limit: usize,
) -> Result<Vec<FocusRowV2>, String> {
    let permitted_json =
        serde_json::to_string(permitted).map_err(|_| "world-projection-corrupt")?;
    let mut statement = c
        .prepare(
            "SELECT f.assertion_id,f.entity_id,f.reason,f.objective_assertion_id
               FROM personal_world_focus f
              WHERE f.project_scope=:project AND f.status='active'
                AND f.valid_from<=:now AND (f.valid_until IS NULL OR :now<f.valid_until)
                AND f.assertion_id IN (SELECT value FROM json_each(:permitted))
              ORDER BY CASE f.reason WHEN 'current_work' THEN 0 ELSE 1 END, f.entity_id",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(
            named_params! {
                ":project": project_scope,
                ":now": now,
                ":permitted": permitted_json,
            },
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut focus = Vec::new();
    for (assertion_id, entity_id, reason, objective_assertion_id) in rows {
        if reason == "current_work" {
            match objective_assertion_id.as_deref() {
                Some(id)
                    if ledger.status(id, now) == saaa_personal_state_core::Status::Active
                        && ledger.assertions.get(id).is_some_and(|objective| {
                            objective.access.task_request.as_deref() == Some(project_scope)
                        }) => {}
                _ => continue,
            }
        }
        focus.push(FocusRowV2 {
            assertion_id,
            entity_id,
            reason,
            objective_assertion_id,
        });
        if focus.len() == limit.max(1) {
            break;
        }
    }
    Ok(focus)
}

/// D21: authorization, projection freshness, entity/Goal closure, seed
/// resolution and observation validation. No traversal yet.
pub fn load_query_context_v2(
    c: &Connection,
    input: &ActivateInputV2<'_>,
    ledger: &Ledger,
) -> Result<QueryContextV2, String> {
    authorize_input(input)?;
    let scope = scope_state(c)?;
    if input.access.policy_revision != scope.policy_revision
        || input.access.principal != scope.principal
    {
        return Err("world-scope-denied".into());
    }
    if !project_active(c, input.project_scope)? {
        return Err("world-scope-denied".into());
    }
    let mut context = QueryContextV2 {
        revision: scope.revision,
        as_of_ms: input.now,
        ..Default::default()
    };
    let meta: Option<(u64, u64, u64, i64)> = c
        .query_row(
            "SELECT ledger_revision,input_epoch,policy_revision,projection_version
               FROM personal_world_projection_meta WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .ok();
    let Some((revision, epoch, policy, projection_version)) = meta else {
        context.stale = true;
        context.notices.push(STALE.into());
        return Ok(context);
    };
    // `activate_v2` reads both the v1-only projection (version 1) and the v2
    // projection (version 2); only the old v1 reader is restricted to version 1.
    if !(1..=2).contains(&projection_version)
        || revision != scope.revision
        || epoch != scope.input_epoch
        || policy != scope.policy_revision
    {
        context.stale = true;
        context.notices.push(STALE.into());
        return Ok(context);
    }
    if pending_review(c, input.project_scope)? {
        context.pending = true;
        context.notices.push(PENDING.into());
        return Ok(context);
    }
    context.permitted = permitted_assertions(c, input.access, input.project_scope)?;
    let (entities, aliases) = load_entities(
        c,
        ledger,
        input.project_scope,
        input.now,
        &context.permitted,
    )?;
    let (seeds, notice) = resolve_seeds(input.seeds, &entities, &aliases);
    if let Some(notice) = notice {
        context.notices.push(notice.into());
    }
    context.seeds = seeds;
    context.entities = entities;
    context.aliases = aliases;
    let observations = validate_observations(
        c,
        ledger,
        input.access,
        input.project_scope,
        input.now,
        input.condition_observations,
        input.availability_observations,
    )?;
    for notice in &observations.notices {
        if !context.notices.contains(notice) {
            context.notices.push(notice.clone());
        }
    }
    context.observations = observations;
    context.focus = load_focus(
        c,
        ledger,
        input.project_scope,
        input.now,
        &context.permitted,
        input.limits.capped().nodes,
    )?;
    Ok(context)
}

struct EdgeLoad {
    edges: Vec<WorldEdgeV2>,
    fetched: usize,
    scanned: usize,
    truncated: bool,
    reason: Option<&'static str>,
}

fn flags_allow(flags: IncludeFlags, relation_type: RelationTypeV2) -> bool {
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

fn allowed_relation_types(flags: IncludeFlags) -> Vec<&'static str> {
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
fn load_edges_bfs(
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

fn basis_name(basis: saaa_personal_state_core::world::Basis) -> String {
    match basis {
        saaa_personal_state_core::world::Basis::UserStatement => "user_statement".into(),
        saaa_personal_state_core::world::Basis::ModelHypothesis => "model_hypothesis".into(),
    }
}

fn relation_to_slice(edge: &WorldEdgeV2, observations: &ValidatedObservations) -> SliceRelationV2 {
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

fn seed_text(seed: &WorldSeed) -> String {
    match seed {
        WorldSeed::ExactName(name) => saaa_personal_state_core::world::normalize_name(name),
        WorldSeed::EntityId(id) => id.clone(),
    }
}

fn slice_focus_of(candidate: &FocusCandidate) -> SliceFocusV2 {
    SliceFocusV2 {
        entity_id: candidate.entity_id.clone(),
        reason: candidate.reason.clone(),
        objective_assertion_id: candidate.objective_assertion_id.clone(),
    }
}

fn goal_ids(nodes: &[SliceNodeV2]) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.entity_kind == EntityKindV2::Goal)
        .map(|node| node.entity_id.clone())
        .collect()
}

fn project_ids(nodes: &[SliceNodeV2]) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.entity_kind == EntityKindV2::Project)
        .map(|node| node.entity_id.clone())
        .collect()
}

fn node_of(entity: &EntityV2Row) -> SliceNodeV2 {
    SliceNodeV2 {
        entity_id: entity.entity_id.clone(),
        entity_kind: entity.kind,
        name: entity.name.clone(),
        assertion_id: entity.assertion_id.clone(),
        objective_assertion_id: entity.objective_assertion_id.clone(),
    }
}

struct PathView {
    nodes: Vec<SliceNodeV2>,
    relations: Vec<SliceRelationV2>,
    relevance: Option<RelevancePathV2>,
    causal: Option<CausalPathV2>,
}

fn path_view(
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
    let context = load_query_context_v2(c, input, &ledger)?;
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
