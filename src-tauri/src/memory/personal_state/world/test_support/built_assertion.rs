use super::*;
pub const PROJECT: &str = "project:fixture-saaa";
pub fn memory_db() -> Connection {
    let c = Connection::open_in_memory().expect("in-memory sqlite");
    crate::initialize_database(&c).expect("database initializes");
    c
}
pub fn writer_db() -> SqliteWriter {
    SqliteWriter::from_connection(memory_db())
}
pub fn ensure_scope(c: &Connection, project: &str) {
    let opaque = project.strip_prefix("project:").unwrap_or(project);
    c.execute(
        "INSERT OR REPLACE INTO context_scopes(scope_key,kind,opaque_id,state,created_at)
         VALUES(?1,'project',?2,'active','1')",
        params![project, opaque],
    )
    .expect("scope");
    c.execute(
        "INSERT OR REPLACE INTO context_scope_epochs(scope_key,epoch) VALUES(?1,1)",
        [project],
    )
    .expect("scope epoch");
}
/// Synthetic finalized user conversation source bound to a project scope.
pub fn insert_source(c: &Connection, project: &str, id: &str, text: &str) -> SourceRef {
    ensure_scope(c, project);
    c.execute(
        "INSERT INTO conversation_messages VALUES(?1,?2,'user',?3,?4)",
        params![id, crate::PRIMARY_CONVERSATION_ID, text, now().to_string()],
    )
    .expect("insert message");
    c.execute(
        "INSERT OR REPLACE INTO personal_source_scope_refs(source_id,version,scope_key)
         VALUES(?1,1,?2)",
        params![id, project],
    )
    .expect("scope ref");
    c.execute(
        "UPDATE personal_jobs SET scope_key=?2 WHERE source_sequence=(
           SELECT sequence FROM personal_sources WHERE message_id=?1)",
        params![id, project],
    )
    .expect("job scope");
    let total: u64 = c
        .query_row(
            "SELECT bytes FROM personal_sources WHERE message_id=?1 AND version=1",
            [id],
            |r| r.get(0),
        )
        .expect("bytes");
    let sequence: u64 = c
        .query_row(
            "SELECT sequence FROM personal_sources WHERE message_id=?1 AND version=1",
            [id],
            |r| r.get(0),
        )
        .expect("sequence");
    let chunk = sources::load(c, sequence, 0, total.max(4) as usize).expect("load chunk");
    store::remember_source(c, &chunk.source).expect("remember source");
    sources::finalize(c, &chunk.source).expect("finalize");
    chunk.source
}
pub struct BuiltAssertion {
    pub assertion: Assertion,
    pub payload: Value,
}
pub struct AssertionSpec<'a> {
    pub id: &'a str,
    pub payload_ref: &'a str,
    pub kind: Kind,
    pub semantic_key: String,
    pub evidence: BTreeSet<SourceKey>,
    pub depends_on: BTreeSet<String>,
    pub source: &'a SourceRef,
    pub project: &'a str,
    pub now: i64,
}
pub(crate) fn base_assertion(spec: AssertionSpec<'_>) -> Assertion {
    let mut access = spec.source.access.clone();
    access.task_request = Some(spec.project.to_string());
    Assertion {
        id: spec.id.to_string(),
        kind: spec.kind,
        semantic_key: spec.semantic_key,
        payload_ref: spec.payload_ref.to_string(),
        access,
        evidence: spec.evidence,
        depends_on: spec.depends_on,
        input_dependencies: BTreeSet::from([spec.source.key.clone()]),
        provenance: Provenance {
            model: "fixture".into(),
            release: "fixture".into(),
            extractor_version: "wm1".into(),
            prompt_digest: "fixture".into(),
            schema_version: "wm1".into(),
            config_digest: "fixture".into(),
            runtime_event: None,
        },
        observed_at: spec.source.recorded_at,
        effective_at: spec.now,
        recorded_at: spec.now,
        valid_from: spec.now,
        valid_until: None,
    }
}
pub fn entity_payload(entity_id: &str, kind: EntityKind, name: &str, aliases: &[&str]) -> Value {
    json!({
        "type": "entity",
        "schema_version": 1,
        "entity_id": entity_id,
        "entity_kind": match kind {
            EntityKind::Project => "project",
            EntityKind::Concept => "concept",
            EntityKind::Metric => "metric",
        },
        "name": name,
        "aliases": aliases,
    })
}
pub fn relation_payload(
    from: &str,
    to: &str,
    relation_type: RelationType,
    effect: Option<EffectInput>,
    conditions: &[(&str, &str)],
    basis: Basis,
    stakes: Vec<(SourceKey, Stance)>,
) -> Value {
    json!({
        "type": "relation",
        "schema_version": 1,
        "from_entity_id": from,
        "to_entity_id": to,
        "relation_type": relation_type.as_str(),
        "effect_input": effect.map(|e| e.as_str()),
        "conditions": conditions.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
        "basis": match basis { Basis::UserStatement => "user_statement", Basis::ModelHypothesis => "model_hypothesis" },
        "evidence_stances": stakes.iter().map(|(k, s)| json!({
            "source": {"id": k.id, "version": k.version, "start": k.start, "end": k.end},
            "stance": match s { Stance::Supports => "supports", Stance::Challenges => "challenges", Stance::Context => "context" }
        })).collect::<Vec<_>>(),
    })
}
pub fn focus_payload(entity_id: &str, reason: FocusReason, objective: Option<&str>) -> Value {
    json!({
        "type": "focus",
        "schema_version": 1,
        "entity_id": entity_id,
        "reason": match reason { FocusReason::CurrentWork => "current_work", FocusReason::ExplicitInterest => "explicit_interest" },
        "objective_assertion_id": objective,
    })
}
#[allow(clippy::too_many_arguments)]
pub fn entity_assertion(
    id: &str,
    payload_ref: &str,
    entity_id: &str,
    kind: EntityKind,
    name: &str,
    aliases: &[&str],
    source: &SourceRef,
    project: &str,
    now_ms: i64,
) -> BuiltAssertion {
    BuiltAssertion {
        assertion: base_assertion(AssertionSpec {
            id,
            payload_ref,
            kind: Kind::WorldEntity,
            semantic_key: entity_key(project, entity_id),
            evidence: BTreeSet::from([source.key.clone()]),
            depends_on: BTreeSet::new(),
            source,
            project,
            now: now_ms,
        }),
        payload: entity_payload(entity_id, kind, name, aliases),
    }
}
#[allow(clippy::too_many_arguments)]
pub fn relation_assertion(
    id: &str,
    payload_ref: &str,
    from: &str,
    to: &str,
    relation_type: RelationType,
    effect: Option<EffectInput>,
    conditions: &[(&str, &str)],
    basis: Basis,
    stakes: Vec<(SourceKey, Stance)>,
    source: &SourceRef,
    project: &str,
    depends_on: BTreeSet<String>,
    now_ms: i64,
) -> BuiltAssertion {
    let mut sorted: Vec<(String, String)> = conditions
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    sorted.sort();
    let (key_from, key_to) = if relation_type == RelationType::RelatedTo {
        order_undirected(from, to)
    } else {
        (from.to_string(), to.to_string())
    };
    let key = relation_key(&RelationKeyInput {
        project_scope: project,
        from: &key_from,
        to: &key_to,
        relation_type: relation_type.as_str(),
        effect_input: effect.map(|e| e.as_str()),
        sorted_conditions: &sorted,
        valid_from: now_ms,
        valid_until: None,
    });
    BuiltAssertion {
        assertion: base_assertion(AssertionSpec {
            id,
            payload_ref,
            kind: Kind::WorldRelation,
            semantic_key: key,
            evidence: BTreeSet::from([source.key.clone()]),
            depends_on,
            source,
            project,
            now: now_ms,
        }),
        payload: relation_payload(from, to, relation_type, effect, conditions, basis, stakes),
    }
}
#[allow(clippy::too_many_arguments)]
pub fn focus_assertion(
    id: &str,
    payload_ref: &str,
    entity_id: &str,
    reason: FocusReason,
    objective: Option<&str>,
    source: &SourceRef,
    project: &str,
    depends_on: BTreeSet<String>,
    now_ms: i64,
) -> BuiltAssertion {
    let key = focus_key(
        project,
        entity_id,
        match reason {
            FocusReason::CurrentWork => "current_work",
            FocusReason::ExplicitInterest => "explicit_interest",
        },
    );
    BuiltAssertion {
        assertion: base_assertion(AssertionSpec {
            id,
            payload_ref,
            kind: Kind::WorldFocus,
            semantic_key: key,
            evidence: BTreeSet::from([source.key.clone()]),
            depends_on,
            source,
            project,
            now: now_ms,
        }),
        payload: focus_payload(entity_id, reason, objective),
    }
}
pub fn objective_assertion(
    id: &str,
    payload_ref: &str,
    source: &SourceRef,
    project: &str,
    now_ms: i64,
) -> BuiltAssertion {
    BuiltAssertion {
        assertion: base_assertion(AssertionSpec {
            id,
            payload_ref,
            kind: Kind::Objective,
            semantic_key: format!("objective:{id}"),
            evidence: BTreeSet::from([source.key.clone()]),
            depends_on: BTreeSet::new(),
            source,
            project,
            now: now_ms,
        }),
        payload: json!({"objective": "音声応答の改善"}),
    }
}
/// Commits assertions through the real `store::commit` path so 16 KiB / 32 op
/// limits and the World validation hook are exercised.
pub struct Committer<'a> {
    pub writer: &'a SqliteWriter,
    pub project: &'a str,
}
