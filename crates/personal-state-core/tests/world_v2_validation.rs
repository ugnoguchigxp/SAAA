//! Core v2 semantic validation and condition tests (D10-D15).
use saaa_personal_state_core::world::conditions_v2::*;
use saaa_personal_state_core::world::identity_v2;
use saaa_personal_state_core::world::model::{
    Basis, Condition, EffectInput, EntityTag, EvidenceStance, FocusReason, FocusTag, RelationTag,
    Stance, WorldPayload,
};
use saaa_personal_state_core::world::model_v2::*;
use saaa_personal_state_core::world::validation::WorldError;
use saaa_personal_state_core::world::validation_v2::{
    validate_versioned_patch, VersionedPatchInput,
};
use saaa_personal_state_core::world::versioned::VersionedWorldPayload;
use saaa_personal_state_core::*;
use std::collections::{BTreeMap, BTreeSet};

const SCOPE: &str = "project:p";

fn source() -> SourceKey {
    SourceKey {
        id: "s1".into(),
        version: 1,
        start: 0,
        end: 1,
    }
}

fn access() -> AccessScope {
    AccessScope {
        principal: "owner".into(),
        scope: "owner".into(),
        task_request: Some(SCOPE.into()),
        purposes: BTreeSet::from([Purpose::StateExtract]),
        classification: Classification::Internal,
        policy_revision: 0,
    }
}

fn provenance() -> Provenance {
    Provenance {
        model: "test".into(),
        release: "1".into(),
        extractor_version: "1".into(),
        prompt_digest: "d".into(),
        schema_version: "1".into(),
        config_digest: "c".into(),
        runtime_event: None,
    }
}

fn assertion(id: &str, kind: Kind, payload_ref: &str, semantic_key: String) -> Assertion {
    Assertion {
        id: id.into(),
        kind,
        semantic_key,
        payload_ref: payload_ref.into(),
        access: access(),
        evidence: BTreeSet::from([source()]),
        depends_on: BTreeSet::new(),
        input_dependencies: BTreeSet::new(),
        provenance: provenance(),
        observed_at: 0,
        effective_at: 0,
        recorded_at: 0,
        valid_from: 0,
        valid_until: None,
    }
}

fn transition(id: &str, action: Action) -> Transition {
    Transition {
        id: format!("t_{id}"),
        sequence: 0,
        assertion_id: id.into(),
        action,
        reason_code: "test".into(),
        evidence: BTreeSet::from([source()]),
        input_dependencies: BTreeSet::new(),
        recorded_at: 0,
    }
}

struct World {
    ledger: Ledger,
    existing: BTreeMap<String, VersionedWorldPayload>,
}

impl World {
    fn new() -> Self {
        let mut ledger = Ledger::new("owner".into(), "owner".into(), 0);
        ledger.sources.insert(
            source(),
            SourceRef {
                key: source(),
                digest: "d".into(),
                sequence: 1,
                recorded_at: 0,
                role: SourceRole::User,
                access: access(),
                available: true,
                valid_until: None,
                finalized: true,
            },
        );
        // Active objective o1.
        let objective = assertion("o1", Kind::Objective, "pr_o1", "obj:o1".into());
        ledger.assertions.insert("o1".into(), objective);
        ledger.transitions.push(transition("o1", Action::Assert));
        ledger.transitions.push(transition("o1", Action::Activate));
        Self {
            ledger,
            existing: BTreeMap::new(),
        }
    }

    fn add_entity(&mut self, id: &str, entity_id: &str, kind: EntityKindV2) {
        let payload = EntityPayloadV2 {
            payload_type: EntityTag::Entity,
            schema_version: 2,
            entity_id: entity_id.into(),
            entity_kind: kind,
            name: entity_id.into(),
            aliases: Vec::new(),
            objective_assertion_id: if kind == EntityKindV2::Goal {
                Some("o1".into())
            } else {
                None
            },
        };
        let key = identity_v2::entity_key_v2(SCOPE, entity_id);
        let mut a = assertion(id, Kind::WorldEntity, &format!("pr_{id}"), key);
        if kind == EntityKindV2::Goal {
            a.depends_on.insert("o1".into());
        }
        self.ledger.assertions.insert(id.into(), a);
        self.ledger.transitions.push(transition(id, Action::Assert));
        self.ledger
            .transitions
            .push(transition(id, Action::Activate));
        self.existing.insert(
            format!("pr_{id}"),
            VersionedWorldPayload::V2(WorldPayloadV2::Entity(payload)),
        );
    }

    fn input<'a>(
        &'a self,
        patch: &'a StatePatch,
        payloads: &'a BTreeMap<String, VersionedWorldPayload>,
    ) -> VersionedPatchInput<'a> {
        VersionedPatchInput {
            ledger: &self.ledger,
            patch,
            payloads,
            existing: &self.existing,
            project_scope: SCOPE,
            now: 100,
        }
    }
}

fn v2_entity_payload(
    entity_id: &str,
    kind: EntityKindV2,
    objective: Option<&str>,
) -> EntityPayloadV2 {
    EntityPayloadV2 {
        payload_type: EntityTag::Entity,
        schema_version: 2,
        entity_id: entity_id.into(),
        entity_kind: kind,
        name: entity_id.into(),
        aliases: Vec::new(),
        objective_assertion_id: objective.map(|s| s.to_string()),
    }
}

fn v2_relation_payload(from: &str, to: &str, relation_type: RelationTypeV2) -> RelationPayloadV2 {
    RelationPayloadV2 {
        payload_type: RelationTag::Relation,
        schema_version: 2,
        from_entity_id: from.into(),
        to_entity_id: to.into(),
        relation_type,
        effect_input: if matches!(
            relation_type,
            RelationTypeV2::Increases
                | RelationTypeV2::Decreases
                | RelationTypeV2::Causes
                | RelationTypeV2::Enables
                | RelationTypeV2::Inhibits
        ) {
            Some(EffectInput::Intervention)
        } else {
            None
        },
        conditions: Vec::new(),
        comparison_id: None,
        basis: Basis::ModelHypothesis,
        evidence_stances: vec![EvidenceStance {
            source: source(),
            stance: Stance::Context,
        }],
        target_direction: None,
        correlation_sign: if relation_type == RelationTypeV2::CorrelatesWith {
            Some(CorrelationSign::Negative)
        } else {
            None
        },
        epistemic: Epistemic::Hypothesis,
        confidence: None,
        strength: None,
        assessment_refs: Vec::new(),
        mechanism: MechanismState::Unassessed,
        outcome_update: None,
    }
}

#[test]
fn d10_goal_entity_accepts_only_active_objective() {
    let mut world = World::new();
    let payload = v2_entity_payload("g1", EntityKindV2::Goal, Some("o1"));
    let key = identity_v2::entity_key_v2(SCOPE, "g1");
    let mut a = assertion("a_g1", Kind::WorldEntity, "pr_g1", key);
    a.depends_on.insert("o1".into());
    let patch = StatePatch {
        id: "p1".into(),
        base_revision: 0,
        input_epoch: 0,
        policy_revision: 0,
        fence: "f".into(),
        assertions: vec![a],
        transitions: vec![
            transition("a_g1", Action::Assert),
            transition("a_g1", Action::Activate),
        ],
        coverage: Vec::new(),
    };
    let payloads = BTreeMap::from([(
        "pr_g1".to_string(),
        VersionedWorldPayload::V2(WorldPayloadV2::Entity(payload)),
    )]);
    assert!(validate_versioned_patch(&world.input(&patch, &payloads)).is_ok());

    // A different kind cannot be a goal's objective.
    world.ledger.assertions.insert(
        "o1".into(),
        assertion("o1", Kind::Decision, "pr_o2", "dec:o1".into()),
    );
    assert_eq!(
        validate_versioned_patch(&world.input(&patch, &payloads)).unwrap_err(),
        WorldError::InvalidReference
    );
}

#[test]
fn d10_goal_objective_in_other_project_is_scope_denied() {
    let world = World::new();
    let payload = v2_entity_payload("g1", EntityKindV2::Goal, Some("o1"));
    let key = identity_v2::entity_key_v2(SCOPE, "g1");
    let mut a = assertion("a_g1", Kind::WorldEntity, "pr_g1", key);
    a.depends_on.insert("o1".into());
    let patch = StatePatch {
        id: "p1".into(),
        base_revision: 0,
        input_epoch: 0,
        policy_revision: 0,
        fence: "f".into(),
        assertions: vec![a],
        transitions: Vec::new(),
        coverage: Vec::new(),
    };
    // Objective belongs to a different task request.
    let mut ledger = world.ledger.clone();
    let objective = ledger.assertions.get_mut("o1").unwrap();
    objective.access.task_request = Some("project:q".into());
    let payloads = BTreeMap::from([(
        "pr_g1".to_string(),
        VersionedWorldPayload::V2(WorldPayloadV2::Entity(payload)),
    )]);
    let input = VersionedPatchInput {
        ledger: &ledger,
        patch: &patch,
        payloads: &payloads,
        existing: &world.existing,
        project_scope: SCOPE,
        now: 100,
    };
    assert_eq!(
        validate_versioned_patch(&input).unwrap_err(),
        WorldError::ScopeDenied
    );
}

fn relation_patch(
    world: &World,
    id: &str,
    payload: RelationPayloadV2,
    supersede_existing: Option<&str>,
) -> (StatePatch, BTreeMap<String, VersionedWorldPayload>) {
    let key = identity_v2::relation_key_from_payload(SCOPE, &payload, 0, None);
    let mut a = assertion(id, Kind::WorldRelation, &format!("pr_{id}"), key);
    a.depends_on.insert(format!("c_{}", payload.from_entity_id));
    a.depends_on.insert(format!("c_{}", payload.to_entity_id));
    let mut transitions = vec![
        transition(id, Action::Assert),
        transition(id, Action::Activate),
    ];
    if let Some(existing) = supersede_existing {
        transitions.push(Transition {
            id: format!("t_sup_{existing}"),
            sequence: 0,
            assertion_id: existing.into(),
            action: Action::Supersede { by: id.into() },
            reason_code: "test".into(),
            evidence: BTreeSet::from([source()]),
            input_dependencies: BTreeSet::new(),
            recorded_at: 0,
        });
    }
    let _ = world;
    let patch = StatePatch {
        id: format!("p_{id}"),
        base_revision: 0,
        input_epoch: 0,
        policy_revision: 0,
        fence: "f".into(),
        assertions: vec![a],
        transitions,
        coverage: Vec::new(),
    };
    let payloads = BTreeMap::from([(
        format!("pr_{id}"),
        VersionedWorldPayload::V2(WorldPayloadV2::Relation(payload)),
    )]);
    (patch, payloads)
}

fn causal_world() -> World {
    let mut world = World::new();
    world.add_entity("c_c1", "c1", EntityKindV2::Concept);
    world.add_entity("c_c2", "c2", EntityKindV2::Concept);
    world.add_entity("c_m1", "m1", EntityKindV2::Metric);
    world.add_entity("c_m2", "m2", EntityKindV2::Metric);
    world.add_entity("c_g1", "g1", EntityKindV2::Goal);
    world.add_entity("c_p1", "p1", EntityKindV2::Project);
    world.add_entity("c_a1", "a1", EntityKindV2::Actor);
    world
}

#[test]
fn d11_increases_requires_concept_or_metric_to_metric() {
    let world = causal_world();
    let mut payload = v2_relation_payload("c1", "m1", RelationTypeV2::Increases);
    payload.effect_input = Some(EffectInput::Intervention);
    let (patch, payloads) = relation_patch(&world, "r1", payload.clone(), None);
    assert!(validate_versioned_patch(&world.input(&patch, &payloads)).is_ok());

    // project as causal source is rejected.
    payload.from_entity_id = "p1".into();
    let (patch, payloads) = relation_patch(&world, "r2", payload, None);
    assert_eq!(
        validate_versioned_patch(&world.input(&patch, &payloads)).unwrap_err(),
        WorldError::InvalidPayload
    );
}

#[test]
fn d11_serves_goal_metric_requires_target_direction() {
    let world = causal_world();
    let payload = v2_relation_payload("m1", "g1", RelationTypeV2::ServesGoal);
    let (patch, payloads) = relation_patch(&world, "r1", payload, None);
    assert_eq!(
        validate_versioned_patch(&world.input(&patch, &payloads)).unwrap_err(),
        WorldError::InvalidPayload
    );

    let mut with_direction = v2_relation_payload("m1", "g1", RelationTypeV2::ServesGoal);
    with_direction.target_direction = Some(TargetDirection::LowerIsBetter);
    let (patch, payloads) = relation_patch(&world, "r2", with_direction, None);
    assert!(validate_versioned_patch(&world.input(&patch, &payloads)).is_ok());
}

#[test]
fn d11_correlates_with_rejects_effect_input() {
    let world = causal_world();
    let mut payload = v2_relation_payload("c1", "m1", RelationTypeV2::CorrelatesWith);
    payload.effect_input = Some(EffectInput::Intervention);
    let (patch, payloads) = relation_patch(&world, "r1", payload, None);
    assert_eq!(
        validate_versioned_patch(&world.input(&patch, &payloads)).unwrap_err(),
        WorldError::InvalidPayload
    );
}

#[test]
fn d11_causes_requires_concept_endpoints() {
    let world = causal_world();
    // concept -> metric is not a valid causes pair.
    let payload = v2_relation_payload("c1", "m1", RelationTypeV2::Causes);
    let (patch, payloads) = relation_patch(&world, "r1", payload, None);
    assert_eq!(
        validate_versioned_patch(&world.input(&patch, &payloads)).unwrap_err(),
        WorldError::InvalidPayload
    );
}

#[test]
fn d12_confidence_requires_evidence_and_supported_is_rejected() {
    let world = causal_world();
    let mut payload = v2_relation_payload("c1", "m1", RelationTypeV2::Increases);
    payload.effect_input = Some(EffectInput::Intervention);
    payload.confidence = Some(Confidence {
        value: 800,
        method: ConfidenceMethod::ManualV1,
    });
    payload.assessment_refs = vec![source()];
    let (patch, payloads) = relation_patch(&world, "r1", payload.clone(), None);
    assert!(validate_versioned_patch(&world.input(&patch, &payloads)).is_ok());

    // assessment refs not declared in the assertion are rejected.
    payload.assessment_refs = vec![SourceKey {
        id: "other".into(),
        version: 1,
        start: 0,
        end: 1,
    }];
    let (patch, payloads) = relation_patch(&world, "r2", payload.clone(), None);
    assert_eq!(
        validate_versioned_patch(&world.input(&patch, &payloads)).unwrap_err(),
        WorldError::InvalidReference
    );

    // supported is a reserved value.
    let mut supported = payload.clone();
    supported.assessment_refs = vec![source()];
    supported.epistemic = Epistemic::Supported;
    let (patch, payloads) = relation_patch(&world, "r3", supported, None);
    assert_eq!(
        validate_versioned_patch(&world.input(&patch, &payloads)).unwrap_err(),
        WorldError::InvalidPayload
    );
}

#[test]
fn d13_version_crossing_duplicate_requires_supersede() {
    let world = causal_world();
    // Existing v1 relation c1 -> m1 increases.
    let v1 = saaa_personal_state_core::world::model::RelationPayload {
        payload_type: RelationTag::Relation,
        schema_version: 1,
        from_entity_id: "c1".into(),
        to_entity_id: "m1".into(),
        relation_type: saaa_personal_state_core::world::model::RelationType::Increases,
        effect_input: Some(EffectInput::Intervention),
        conditions: vec![Condition {
            key: "config".into(),
            value: "a".into(),
        }],
        basis: Basis::ModelHypothesis,
        evidence_stances: vec![EvidenceStance {
            source: source(),
            stance: Stance::Context,
        }],
    };
    let mut ledger = world.ledger.clone();
    let mut existing = world.existing.clone();
    let mut old = assertion("r_old", Kind::WorldRelation, "pr_r_old", "wm1:old".into());
    old.depends_on.insert("c_c1".into());
    old.depends_on.insert("c_m1".into());
    ledger.assertions.insert("r_old".into(), old);
    ledger.transitions.push(transition("r_old", Action::Assert));
    ledger
        .transitions
        .push(transition("r_old", Action::Activate));
    existing.insert(
        "pr_r_old".into(),
        VersionedWorldPayload::V1(WorldPayload::Relation(v1)),
    );

    let mut payload = v2_relation_payload("c1", "m1", RelationTypeV2::Increases);
    payload.effect_input = Some(EffectInput::Intervention);
    payload.conditions = vec![Condition {
        key: "config".into(),
        value: "a".into(),
    }];
    let (patch, payloads) = relation_patch(&world, "r_new", payload.clone(), None);
    let input = VersionedPatchInput {
        ledger: &ledger,
        patch: &patch,
        payloads: &payloads,
        existing: &existing,
        project_scope: SCOPE,
        now: 100,
    };
    assert_eq!(
        validate_versioned_patch(&input).unwrap_err(),
        WorldError::DuplicateIdentity
    );

    let (patch, payloads) = relation_patch(&world, "r_new", payload, Some("r_old"));
    let input = VersionedPatchInput {
        ledger: &ledger,
        patch: &patch,
        payloads: &payloads,
        existing: &existing,
        project_scope: SCOPE,
        now: 100,
    };
    assert!(validate_versioned_patch(&input).is_ok());
}

#[test]
fn d13_two_new_same_identity_are_duplicates() {
    let world = causal_world();
    let payload = v2_relation_payload("c1", "m1", RelationTypeV2::Increases);
    let (patch_a, payloads_a) = relation_patch(&world, "r1", payload.clone(), None);
    let (patch_b, _) = relation_patch(&world, "r2", payload, None);
    let mut patch = patch_a.clone();
    patch.assertions.extend(patch_b.assertions.clone());
    patch.transitions.extend(patch_b.transitions.clone());
    let mut payloads = payloads_a.clone();
    payloads.insert(
        "pr_r2".into(),
        VersionedWorldPayload::V2(WorldPayloadV2::Relation(v2_relation_payload(
            "c1",
            "m1",
            RelationTypeV2::Increases,
        ))),
    );
    let input = world.input(&patch, &payloads);
    assert_eq!(
        validate_versioned_patch(&input).unwrap_err(),
        WorldError::DuplicateIdentity
    );
}

#[test]
fn d14_condition_three_valued_evaluation() {
    let conditions = vec![Condition {
        key: "config".into(),
        value: "a".into(),
    }];
    let observation = |value: &str, evidence: &str| ConditionObservation {
        relation_assertion_id: "r1".into(),
        key: "config".into(),
        value: value.into(),
        evidence: SourceKey {
            id: evidence.into(),
            version: 1,
            start: 0,
            end: 1,
        },
        valid_from_ms: 0,
        valid_until_ms: Some(200),
    };
    assert_eq!(
        evaluate_conditions("r1", &conditions, &[observation("a", "s1")]).aggregate,
        saaa_personal_state_core::world::slice_v2::ConditionStateV2::Satisfied
    );
    assert_eq!(
        evaluate_conditions("r1", &conditions, &[observation("b", "s1")]).aggregate,
        saaa_personal_state_core::world::slice_v2::ConditionStateV2::Violated
    );
    assert_eq!(
        evaluate_conditions(
            "r1",
            &conditions,
            &[observation("a", "s1"), observation("b", "s2")]
        )
        .aggregate,
        saaa_personal_state_core::world::slice_v2::ConditionStateV2::Unknown
    );
    // Observations keyed to another relation never apply.
    let mut other = observation("a", "s1");
    other.relation_assertion_id = "r2".into();
    assert_eq!(
        evaluate_conditions("r1", &conditions, &[other]).aggregate,
        saaa_personal_state_core::world::slice_v2::ConditionStateV2::Unknown
    );
    assert_eq!(
        evaluate_conditions("r1", &[], &[observation("a", "s1")]).aggregate,
        saaa_personal_state_core::world::slice_v2::ConditionStateV2::Unknown
    );
}

#[test]
fn d15_availability_unknown_without_observation() {
    let available = AvailabilityObservation {
        entity_id: "b".into(),
        value: AvailabilityValue::Available,
        evidence: source(),
        valid_from_ms: 0,
        valid_until_ms: None,
    };
    assert_eq!(
        evaluate_availability("b", std::slice::from_ref(&available)).state,
        saaa_personal_state_core::world::slice_v2::AvailabilityStateV2::Available
    );
    let mut unavailable = available.clone();
    unavailable.value = AvailabilityValue::Unavailable;
    assert_eq!(
        evaluate_availability("b", &[available, unavailable]).state,
        saaa_personal_state_core::world::slice_v2::AvailabilityStateV2::Unknown
    );
    assert_eq!(
        evaluate_availability("b", &[]).state,
        saaa_personal_state_core::world::slice_v2::AvailabilityStateV2::Unknown
    );
}

#[test]
fn d06_v2_focus_roundtrip() {
    let payload = FocusPayloadV2 {
        payload_type: FocusTag::Focus,
        schema_version: 2,
        entity_id: "e1".into(),
        reason: FocusReason::ExplicitInterest,
        objective_assertion_id: None,
    };
    let value = serde_json::to_value(&payload).unwrap();
    let decoded = WorldPayloadV2::decode("world_focus", &value).unwrap();
    assert_eq!(decoded, WorldPayloadV2::Focus(payload));
}

fn relation_case(
    from: &str,
    to: &str,
    relation_type: RelationTypeV2,
    effect: Option<EffectInput>,
    target: Option<TargetDirection>,
    sign: Option<CorrelationSign>,
) -> RelationPayloadV2 {
    let mut payload = v2_relation_payload(from, to, relation_type);
    payload.effect_input = effect;
    payload.target_direction = target;
    payload.correlation_sign = sign;
    if relation_type == RelationTypeV2::CorrelatesWith {
        payload.correlation_sign = Some(CorrelationSign::Positive);
    }
    payload
}

#[test]
fn d11_all_twelve_relation_types_have_a_valid_case() {
    let world = causal_world();
    let cases: Vec<(&str, RelationPayloadV2)> = vec![
        (
            "related_to",
            relation_case("c1", "m1", RelationTypeV2::RelatedTo, None, None, None),
        ),
        (
            "part_of",
            relation_case("c1", "c2", RelationTypeV2::PartOf, None, None, None),
        ),
        (
            "important_for",
            relation_case("m1", "g1", RelationTypeV2::ImportantFor, None, None, None),
        ),
        (
            "increases_concept",
            relation_case(
                "c1",
                "m1",
                RelationTypeV2::Increases,
                Some(EffectInput::Intervention),
                None,
                None,
            ),
        ),
        (
            "increases_metric",
            relation_case(
                "m1",
                "m2",
                RelationTypeV2::Increases,
                Some(EffectInput::QuantityIncrease),
                None,
                None,
            ),
        ),
        (
            "decreases",
            relation_case(
                "c1",
                "m1",
                RelationTypeV2::Decreases,
                Some(EffectInput::Intervention),
                None,
                None,
            ),
        ),
        (
            "causes",
            relation_case(
                "c1",
                "c2",
                RelationTypeV2::Causes,
                Some(EffectInput::Intervention),
                None,
                None,
            ),
        ),
        (
            "enables",
            relation_case(
                "c1",
                "c2",
                RelationTypeV2::Enables,
                Some(EffectInput::Intervention),
                None,
                None,
            ),
        ),
        (
            "inhibits",
            relation_case(
                "c1",
                "c2",
                RelationTypeV2::Inhibits,
                Some(EffectInput::Intervention),
                None,
                None,
            ),
        ),
        (
            "has_goal",
            relation_case("p1", "g1", RelationTypeV2::HasGoal, None, None, None),
        ),
        (
            "serves_goal",
            relation_case(
                "m1",
                "g1",
                RelationTypeV2::ServesGoal,
                None,
                Some(TargetDirection::LowerIsBetter),
                None,
            ),
        ),
        (
            "correlates_with",
            relation_case("m1", "m2", RelationTypeV2::CorrelatesWith, None, None, None),
        ),
        (
            "depends_on",
            relation_case("c1", "c2", RelationTypeV2::DependsOn, None, None, None),
        ),
    ];
    for (name, payload) in cases {
        let (patch, payloads) = relation_patch(&world, &format!("r_{name}"), payload, None);
        assert!(
            validate_versioned_patch(&world.input(&patch, &payloads)).is_ok(),
            "valid case rejected: {name}"
        );
    }
}

#[test]
fn d11_negative_endpoint_cases_are_rejected() {
    let world = causal_world();
    let cases: Vec<(&str, RelationPayloadV2)> = vec![
        (
            "causes_metric",
            relation_case(
                "m1",
                "m2",
                RelationTypeV2::Causes,
                Some(EffectInput::Intervention),
                None,
                None,
            ),
        ),
        (
            "serves_goal_concept_with_direction",
            relation_case(
                "c1",
                "g1",
                RelationTypeV2::ServesGoal,
                None,
                Some(TargetDirection::LowerIsBetter),
                None,
            ),
        ),
        (
            "correlates_non_metric",
            relation_case("c1", "m1", RelationTypeV2::CorrelatesWith, None, None, None),
        ),
        (
            "depends_on_goal",
            relation_case("c1", "g1", RelationTypeV2::DependsOn, None, None, None),
        ),
        (
            "has_goal_from_concept",
            relation_case("c1", "g1", RelationTypeV2::HasGoal, None, None, None),
        ),
        (
            "increases_missing_effect",
            relation_case("c1", "m1", RelationTypeV2::Increases, None, None, None),
        ),
    ];
    for (name, payload) in cases {
        let (patch, payloads) = relation_patch(&world, &format!("r_{name}"), payload, None);
        assert!(
            validate_versioned_patch(&world.input(&patch, &payloads)).is_err(),
            "invalid case accepted: {name}"
        );
    }
}

impl World {
    fn add_relation_v2(&mut self, id: &str, mut payload: RelationPayloadV2) {
        if payload.assessment_refs.is_empty() {
            payload.assessment_refs = vec![source()];
        }
        let key = identity_v2::relation_key_from_payload(SCOPE, &payload, 0, None);
        let mut a = assertion(id, Kind::WorldRelation, &format!("pr_{id}"), key);
        a.depends_on.insert(format!("c_{}", payload.from_entity_id));
        a.depends_on.insert(format!("c_{}", payload.to_entity_id));
        self.ledger.assertions.insert(id.into(), a);
        self.ledger.transitions.push(transition(id, Action::Assert));
        self.ledger
            .transitions
            .push(transition(id, Action::Activate));
        self.existing.insert(
            format!("pr_{id}"),
            VersionedWorldPayload::V2(WorldPayloadV2::Relation(payload)),
        );
    }
}

fn outcome_payload(prior: &RelationPayloadV2, confidence_value: u16) -> RelationPayloadV2 {
    let mut next = prior.clone();
    next.epistemic = Epistemic::Disputed;
    next.confidence = Some(Confidence {
        value: confidence_value,
        method: ConfidenceMethod::CounterevidenceV1,
    });
    next.outcome_update = None;
    next
}

#[test]
fn d35_counterevidence_score_is_recomputed_and_a_forged_value_is_rejected() {
    use saaa_personal_state_core::world::model_v2::{
        Confidence, ConfidenceMethod, EffectDirection, OutcomeUpdate,
    };
    let mut world = causal_world();
    world.add_relation_v2(
        "prior",
        RelationPayloadV2 {
            payload_type: RelationTag::Relation,
            schema_version: 2,
            from_entity_id: "c1".into(),
            to_entity_id: "m1".into(),
            relation_type: RelationTypeV2::Increases,
            effect_input: Some(EffectInput::Intervention),
            conditions: vec![Condition {
                key: "config".into(),
                value: "a".into(),
            }],
            comparison_id: Some("cmp".into()),
            basis: Basis::ModelHypothesis,
            evidence_stances: vec![EvidenceStance {
                source: source(),
                stance: Stance::Context,
            }],
            target_direction: None,
            correlation_sign: None,
            epistemic: Epistemic::Hypothesis,
            confidence: Some(Confidence {
                value: 800,
                method: ConfidenceMethod::ManualV1,
            }),
            strength: None,
            assessment_refs: vec![source()],
            mechanism: MechanismState::Unassessed,
            outcome_update: None,
        },
    );
    let prior = match world.existing.get("pr_prior").expect("prior") {
        VersionedWorldPayload::V2(WorldPayloadV2::Relation(p)) => p.clone(),
        _ => panic!("relation"),
    };
    let update = OutcomeUpdate {
        prior_assertion_id: "prior".into(),
        outcome_source: source(),
        prediction_source: source(),
        comparison_id: "cmp".into(),
        expected: EffectDirection::Increase,
        actual: EffectDirection::Decrease,
        predicted_at_ms: 10,
        observed_at_ms: 20,
    };

    let forged = outcome_payload(&prior, 700);
    let mut forged = forged;
    forged.outcome_update = Some(update.clone());
    let (patch, payloads) = relation_patch(&world, "r_forged", forged, Some("prior"));
    let mut patch = patch;
    patch
        .transitions
        .push(transition("r_forged", Action::Dispute));
    assert_eq!(
        validate_versioned_patch(&world.input(&patch, &payloads)).unwrap_err(),
        WorldError::InvalidPayload
    );

    let correct = outcome_payload(&prior, 640);
    let mut correct = correct;
    correct.outcome_update = Some(update);
    let (patch, payloads) = relation_patch(&world, "r_correct", correct, Some("prior"));
    let mut patch = patch;
    patch
        .transitions
        .push(transition("r_correct", Action::Dispute));
    assert!(validate_versioned_patch(&world.input(&patch, &payloads)).is_ok());
}
