use super::*;
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
pub(super) struct ScopeState {
    pub(super) revision: u64,
    pub(super) input_epoch: u64,
    pub(super) policy_revision: u64,
    pub(super) principal: String,
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
pub(super) fn classification_level(value: Classification) -> i64 {
    match value {
        Classification::Public => 0,
        Classification::Internal => 1,
        Classification::Confidential => 2,
        Classification::Restricted => 3,
    }
}
pub(super) fn purpose_name(value: Purpose) -> &'static str {
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
pub(super) fn scope_state(c: &Connection) -> Result<ScopeState, String> {
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
pub(super) fn project_active(c: &Connection, project_scope: &str) -> Result<bool, String> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM context_scopes WHERE scope_key=?1 AND state='active')",
        [project_scope],
        |r| r.get(0),
    )
    .map_err(database_error)
}
/// Non-erased World assertions whose stored AccessScope permits this request (R1).
pub(super) fn permitted_assertions(
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
pub(super) fn pending_review(c: &Connection, project_scope: &str) -> Result<bool, String> {
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
pub(super) struct EntityRow {
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) canonical_name: String,
}
type EntityIndex = (
    BTreeMap<String, EntityRow>,
    BTreeMap<String, BTreeSet<String>>,
);
/// Loads only permitted, currently-active entities and their aliases. A time-only
/// expiry therefore removes an entity, its aliases and every edge that touches it (R2).
pub(super) fn load_entities(
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
pub(super) fn resolve_seeds(
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
