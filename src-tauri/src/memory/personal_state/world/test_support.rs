#![cfg(test)]

//! Deterministic World fixtures for adapter tests. No model, no network.

use crate::memory::personal_state::{encode, now, sources, store};
use crate::persistence::sqlite::SqliteWriter;
use rusqlite::{params, Connection};
use saaa_personal_state_core::world::*;
use saaa_personal_state_core::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

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

fn base_assertion(spec: AssertionSpec<'_>) -> Assertion {
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

impl Committer<'_> {
    pub fn commit(&mut self, fence: &str, built: Vec<BuiltAssertion>) -> Result<(), String> {
        self.commit_inner(fence, built, true)
    }

    /// Commits assertions as Candidate only (no Activate), for boundary tests.
    pub fn commit_candidate(
        &mut self,
        fence: &str,
        built: Vec<BuiltAssertion>,
    ) -> Result<(), String> {
        self.commit_inner(fence, built, false)
    }

    fn commit_inner(
        &mut self,
        fence: &str,
        built: Vec<BuiltAssertion>,
        activate: bool,
    ) -> Result<(), String> {
        let now_ms = now();
        let project = self.project.to_string();
        self.writer.write(|c| {
            let ledger = store::load(c)?;
            let id = crate::new_id("patch");
            let mut assertions = Vec::new();
            let mut payloads: BTreeMap<String, Value> = BTreeMap::new();
            for item in built {
                payloads.insert(item.assertion.payload_ref.clone(), item.payload);
                assertions.push(item.assertion);
            }
            // Timestamps must equal the commit clock (`validate_assertion`).
            for assertion in assertions.iter_mut() {
                assertion.recorded_at = now_ms;
                assertion.effective_at = now_ms;
            }
            let inputs: BTreeSet<SourceKey> = assertions
                .iter()
                .flat_map(|a| a.input_dependencies.iter().cloned())
                .collect();
            let actions: &[Action] = if activate {
                &[Action::Assert, Action::Activate]
            } else {
                &[Action::Assert]
            };
            let mut transitions = Vec::new();
            let mut sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
            for assertion in &assertions {
                for action in actions {
                    transitions.push(Transition {
                        id: crate::new_id("transition"),
                        sequence,
                        assertion_id: assertion.id.clone(),
                        action: action.clone(),
                        reason_code: "world-fixture".into(),
                        evidence: assertion.evidence.clone(),
                        input_dependencies: assertion.input_dependencies.clone(),
                        recorded_at: now_ms,
                    });
                    sequence += 1;
                }
            }
            let patch = StatePatch {
                id: id.clone(),
                base_revision: ledger.revision,
                input_epoch: ledger.input_epoch,
                policy_revision: ledger.policy_revision,
                fence: fence.to_string(),
                assertions,
                transitions,
                coverage: Vec::new(),
            };
            let payload_bytes = payloads
                .iter()
                .map(|(k, v)| Ok((k.clone(), encode(v)?.len())))
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            let context = CommitContext {
                access: AccessRequest {
                    principal: &ledger.principal,
                    scope: "primary",
                    task_request: Some(&project),
                    purpose: Purpose::StateExtract,
                    max_classification: Classification::Confidential,
                    policy_revision: ledger.policy_revision,
                    authorized: true,
                },
                enabled: true,
                now: now_ms,
                live_fence: fence,
                issued_patch_id: &id,
                issued_assertion_ids: patch.assertions.iter().map(|a| a.id.clone()).collect(),
                issued_payload_bytes: payload_bytes,
                evidence_allowlist: inputs.clone(),
                input_dependencies: inputs,
            };
            store::commit(c, &patch, &context, &payloads)?;
            Ok(())
        })
    }

    /// Commit new assertions plus a Supersede transition to an existing one,
    /// in a single patch (the relation replacement path).
    pub fn commit_superseding(
        &mut self,
        fence: &str,
        built: Vec<BuiltAssertion>,
        supersede: &str,
    ) -> Result<(), String> {
        let now_ms = now();
        let project = self.project.to_string();
        self.writer.write(|c| {
            let ledger = store::load(c)?;
            let id = crate::new_id("patch");
            let mut assertions = Vec::new();
            let mut payloads: BTreeMap<String, Value> = BTreeMap::new();
            for item in built {
                payloads.insert(item.assertion.payload_ref.clone(), item.payload);
                assertions.push(item.assertion);
            }
            for assertion in assertions.iter_mut() {
                assertion.recorded_at = now_ms;
                assertion.effective_at = now_ms;
            }
            let inputs: BTreeSet<SourceKey> = assertions
                .iter()
                .flat_map(|a| a.input_dependencies.iter().cloned())
                .chain(
                    ledger
                        .assertions
                        .get(supersede)
                        .into_iter()
                        .flat_map(|a| a.input_dependencies.iter().cloned()),
                )
                .collect();
            let mut transitions = Vec::new();
            let mut sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
            for assertion in &assertions {
                transitions.push(Transition {
                    id: crate::new_id("transition"),
                    sequence,
                    assertion_id: assertion.id.clone(),
                    action: Action::Assert,
                    reason_code: "world-fixture".into(),
                    evidence: assertion.evidence.clone(),
                    input_dependencies: inputs.clone(),
                    recorded_at: now_ms,
                });
                sequence += 1;
            }
            transitions.push(Transition {
                id: crate::new_id("transition"),
                sequence,
                assertion_id: supersede.to_string(),
                action: Action::Supersede {
                    by: assertions.first().expect("replacement").id.clone(),
                },
                reason_code: "world-fixture".into(),
                evidence: ledger.assertions[supersede].evidence.clone(),
                input_dependencies: inputs.clone(),
                recorded_at: now_ms,
            });
            sequence += 1;
            for assertion in &assertions {
                transitions.push(Transition {
                    id: crate::new_id("transition"),
                    sequence,
                    assertion_id: assertion.id.clone(),
                    action: Action::Activate,
                    reason_code: "world-fixture".into(),
                    evidence: assertion.evidence.clone(),
                    input_dependencies: inputs.clone(),
                    recorded_at: now_ms,
                });
                sequence += 1;
            }
            let patch = StatePatch {
                id: id.clone(),
                base_revision: ledger.revision,
                input_epoch: ledger.input_epoch,
                policy_revision: ledger.policy_revision,
                fence: fence.to_string(),
                assertions,
                transitions,
                coverage: Vec::new(),
            };
            let payload_bytes = payloads
                .iter()
                .map(|(k, v)| Ok((k.clone(), encode(v)?.len())))
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            let context = CommitContext {
                access: AccessRequest {
                    principal: &ledger.principal,
                    scope: "primary",
                    task_request: Some(&project),
                    purpose: Purpose::StateExtract,
                    max_classification: Classification::Confidential,
                    policy_revision: ledger.policy_revision,
                    authorized: true,
                },
                enabled: true,
                now: now_ms,
                live_fence: fence,
                issued_patch_id: &id,
                issued_assertion_ids: patch.assertions.iter().map(|a| a.id.clone()).collect(),
                issued_payload_bytes: payload_bytes,
                evidence_allowlist: inputs.clone(),
                input_dependencies: inputs,
            };
            store::commit(c, &patch, &context, &payloads)?;
            Ok(())
        })
    }

    /// Commit an explicit transition patch (supersede/retract/dispute).
    pub fn transition(&mut self, fence: &str, target: &str, action: Action) -> Result<(), String> {
        let now_ms = now();
        let project = self.project.to_string();
        self.writer.write(|c| {
            let ledger = store::load(c)?;
            let existing = ledger
                .assertions
                .get(target)
                .ok_or("world-fixture-target")?;
            let inputs = existing.input_dependencies.clone();
            let id = crate::new_id("patch");
            let sequence = ledger.transitions.last().map_or(1, |t| t.sequence + 1);
            let patch = StatePatch {
                id: id.clone(),
                base_revision: ledger.revision,
                input_epoch: ledger.input_epoch,
                policy_revision: ledger.policy_revision,
                fence: fence.to_string(),
                assertions: Vec::new(),
                transitions: vec![Transition {
                    id: crate::new_id("transition"),
                    sequence,
                    assertion_id: target.to_string(),
                    action: action.clone(),
                    reason_code: "world-fixture".into(),
                    evidence: existing.evidence.clone(),
                    input_dependencies: inputs.clone(),
                    recorded_at: now_ms,
                }],
                coverage: Vec::new(),
            };
            let context = CommitContext {
                access: AccessRequest {
                    principal: &ledger.principal,
                    scope: "primary",
                    task_request: Some(&project),
                    purpose: Purpose::StateExtract,
                    max_classification: Classification::Confidential,
                    policy_revision: ledger.policy_revision,
                    authorized: true,
                },
                enabled: true,
                now: now_ms,
                live_fence: fence,
                issued_patch_id: &id,
                issued_assertion_ids: BTreeSet::new(),
                issued_payload_bytes: BTreeMap::new(),
                evidence_allowlist: inputs.clone(),
                input_dependencies: inputs,
            };
            store::commit(c, &patch, &context, &BTreeMap::new())?;
            Ok(())
        })
    }
}

pub fn access<'a>(ledger: &'a Ledger, project: &'a str) -> AccessRequest<'a> {
    AccessRequest {
        principal: &ledger.principal,
        scope: "primary",
        task_request: Some(project),
        purpose: Purpose::Reasoning,
        max_classification: Classification::Confidential,
        policy_revision: ledger.policy_revision,
        authorized: true,
    }
}

// --- v2 fixtures (D16/D21/D31/D32/D36) -------------------------------------

use saaa_personal_state_core::world::identity_v2;
use saaa_personal_state_core::world::model_v2::EntityKindV2;
use saaa_personal_state_core::world::versioned::{decode_versioned, VersionedWorldPayload};

/// Content-addressed reference for a v2 payload value.
pub fn v2_payload_ref(kind_name: &str, payload: &Value) -> String {
    let decoded = decode_versioned(kind_name, payload).expect("v2 payload decodes");
    match decoded {
        VersionedWorldPayload::V2(payload) => {
            saaa_personal_state_core::world::versioned::payload_ref_v2(
                &payload.canonical_bytes().expect("canonical"),
            )
        }
        VersionedWorldPayload::V1(_) => panic!("expected v2 payload"),
    }
}

pub fn v2_entity_payload(
    entity_id: &str,
    kind: EntityKindV2,
    name: &str,
    aliases: &[&str],
    objective: Option<&str>,
) -> Value {
    json!({
        "type": "entity",
        "schema_version": 2,
        "entity_id": entity_id,
        "entity_kind": match kind {
            EntityKindV2::Project => "project",
            EntityKindV2::Concept => "concept",
            EntityKindV2::Metric => "metric",
            EntityKindV2::Goal => "goal",
            EntityKindV2::Actor => "actor",
        },
        "name": name,
        "aliases": aliases,
        "objective_assertion_id": objective,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn v2_relation_value(
    from: &str,
    to: &str,
    relation_type: &str,
    effect_input: Option<&str>,
    conditions: &[(&str, &str)],
    comparison_id: Option<&str>,
    target_direction: Option<&str>,
    correlation_sign: Option<&str>,
    confidence: Option<(u16, &str)>,
    assessment_refs: &[SourceKey],
    mechanism: Option<&str>,
    outcome_update: Option<Value>,
) -> Value {
    json!({
        "type": "relation",
        "schema_version": 2,
        "from_entity_id": from,
        "to_entity_id": to,
        "relation_type": relation_type,
        "effect_input": effect_input,
        "conditions": conditions.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
        "comparison_id": comparison_id,
        "basis": "model_hypothesis",
        "evidence_stances": assessment_refs.iter().map(|k| json!({
            "source": {"id": k.id, "version": k.version, "start": k.start, "end": k.end},
            "stance": "context"
        })).collect::<Vec<_>>(),
        "target_direction": target_direction,
        "correlation_sign": correlation_sign,
        "epistemic": "hypothesis",
        "confidence": confidence.map(|(value, method)| json!({"value": value, "method": method})),
        "strength": Value::Null,
        "assessment_refs": assessment_refs.iter().map(|k| json!({"id": k.id, "version": k.version, "start": k.start, "end": k.end})).collect::<Vec<_>>(),
        "mechanism": mechanism.unwrap_or("unassessed"),
        "outcome_update": outcome_update,
    })
}

pub fn v2_focus_value(entity_id: &str, reason: &str, objective: Option<&str>) -> Value {
    json!({
        "type": "focus",
        "schema_version": 2,
        "entity_id": entity_id,
        "reason": reason,
        "objective_assertion_id": objective,
    })
}

pub struct V2Spec<'a> {
    pub id: &'a str,
    pub kind_name: &'a str,
    pub payload: Value,
    pub semantic_key: String,
    pub evidence: BTreeSet<SourceKey>,
    pub depends_on: BTreeSet<String>,
    pub source: &'a SourceRef,
    pub project: &'a str,
    pub now: i64,
}

pub fn v2_assertion(spec: V2Spec<'_>) -> BuiltAssertion {
    let payload_ref = v2_payload_ref(spec.kind_name, &spec.payload);
    let kind = match spec.kind_name {
        "world_entity" => Kind::WorldEntity,
        "world_relation" => Kind::WorldRelation,
        _ => Kind::WorldFocus,
    };
    BuiltAssertion {
        assertion: base_assertion(AssertionSpec {
            id: spec.id,
            payload_ref: &payload_ref,
            kind,
            semantic_key: spec.semantic_key,
            evidence: spec.evidence,
            depends_on: spec.depends_on,
            source: spec.source,
            project: spec.project,
            now: spec.now,
        }),
        payload: spec.payload,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn v2_entity_assertion(
    id: &str,
    entity_id: &str,
    kind: EntityKindV2,
    name: &str,
    aliases: &[&str],
    objective: Option<&str>,
    source: &SourceRef,
    project: &str,
    now_ms: i64,
) -> BuiltAssertion {
    let mut depends_on = BTreeSet::new();
    if let Some(objective) = objective {
        depends_on.insert(objective.to_string());
    }
    v2_assertion(V2Spec {
        id,
        kind_name: "world_entity",
        payload: v2_entity_payload(entity_id, kind, name, aliases, objective),
        semantic_key: identity_v2::entity_key_v2(project, entity_id),
        evidence: BTreeSet::from([source.key.clone()]),
        depends_on,
        source,
        project,
        now: now_ms,
    })
}

pub fn v2_relation_assertion(
    id: &str,
    payload: Value,
    source: &SourceRef,
    project: &str,
    depends_on: BTreeSet<String>,
    now_ms: i64,
) -> BuiltAssertion {
    let decoded = decode_versioned("world_relation", &payload).expect("relation");
    let relation = match decoded {
        VersionedWorldPayload::V2(
            saaa_personal_state_core::world::model_v2::WorldPayloadV2::Relation(p),
        ) => p,
        _ => panic!("relation payload"),
    };
    let semantic_key = identity_v2::relation_key_from_payload(project, &relation, now_ms, None);
    v2_assertion(V2Spec {
        id,
        kind_name: "world_relation",
        payload,
        semantic_key,
        evidence: BTreeSet::from([source.key.clone()]),
        depends_on,
        source,
        project,
        now: now_ms,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn v2_focus_assertion(
    id: &str,
    entity_id: &str,
    reason: &str,
    objective: Option<&str>,
    source: &SourceRef,
    project: &str,
    depends_on: BTreeSet<String>,
    now_ms: i64,
) -> BuiltAssertion {
    let semantic_key = identity_v2::focus_key_v2(project, entity_id, reason);
    v2_assertion(V2Spec {
        id,
        kind_name: "world_focus",
        payload: v2_focus_value(entity_id, reason, objective),
        semantic_key,
        evidence: BTreeSet::from([source.key.clone()]),
        depends_on,
        source,
        project,
        now: now_ms,
    })
}
