use super::*;

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
    pub(super) revision: u64,
    pub(super) input_epoch: u64,
    pub(super) policy_revision: u64,
    pub(super) principal: String,
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
