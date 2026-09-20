//! World Model v2 patch validation (D10/D11/D12/D13/D35). Pure: no DB, clock or
//! ID generation. The adapter decodes versioned payloads and passes them in.

use super::identity_v2;
use super::model::{self, FocusReason, Stance};
use super::model_v2::{
    self, check_entity_v2_struct, check_focus_v2_struct, check_relation_v2_struct, EntityKindV2,
    Epistemic, RelationPayloadV2, RelationTypeV2, WorldPayloadV2,
};
use super::validation::{WorldError, MAX_NEW_WORLD_ASSERTIONS, MAX_WORLD_ASSERTIONS};
use super::versioned::{VersionedWorldPayload, WorldView};
use crate::world::identity;
use crate::world::RelationKeyInput;
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

/// The versioned analogue of `WorldPatchInput`.
pub struct VersionedPatchInput<'a> {
    pub ledger: &'a Ledger,
    pub patch: &'a StatePatch,
    pub payloads: &'a BTreeMap<String, VersionedWorldPayload>,
    pub existing: &'a BTreeMap<String, VersionedWorldPayload>,
    pub project_scope: &'a str,
    pub now: i64,
}

fn is_project_scope(value: &str) -> bool {
    value
        .strip_prefix("project:")
        .is_some_and(|id| !id.is_empty())
}

#[derive(Clone)]
struct EntityRef {
    assertion_id: String,
    kind: EntityKindV2,
}

fn new_assertion_by_payload<'a>(world: &'a [&'a Assertion]) -> BTreeMap<&'a str, &'a str> {
    world
        .iter()
        .map(|a| (a.payload_ref.as_str(), a.id.as_str()))
        .collect()
}

fn view_payload(payload: &VersionedWorldPayload, semantic_key: &str) -> WorldView {
    payload.view(semantic_key)
}

fn entity_index(
    input: &VersionedPatchInput<'_>,
    new_refs: &BTreeMap<&str, &str>,
) -> Result<BTreeMap<String, EntityRef>, WorldError> {
    let mut index: BTreeMap<String, EntityRef> = BTreeMap::new();
    for (id, assertion) in &input.ledger.assertions {
        if assertion.kind != Kind::WorldEntity
            || !input.ledger.status(id, input.now).eq(&Status::Active)
            || assertion.access.task_request.as_deref() != Some(input.project_scope)
        {
            continue;
        }
        let Some(payload) = input.existing.get(&assertion.payload_ref) else {
            return Err(WorldError::ProjectionCorrupt);
        };
        let WorldView::Entity(view) = view_payload(payload, &assertion.semantic_key) else {
            return Err(WorldError::ProjectionCorrupt);
        };
        index
            .entry(view.payload.entity_id.clone())
            .or_insert(EntityRef {
                assertion_id: id.clone(),
                kind: view.payload.entity_kind,
            });
    }
    for (payload_ref, assertion_id) in new_refs {
        let Some(payload) = input.payloads.get(*payload_ref) else {
            return Err(WorldError::InvalidPayload);
        };
        if let WorldView::Entity(view) = view_payload(payload, "") {
            index.insert(
                view.payload.entity_id.clone(),
                EntityRef {
                    assertion_id: (*assertion_id).to_string(),
                    kind: view.payload.entity_kind,
                },
            );
        }
    }
    Ok(index)
}

fn planned_status(input: &VersionedPatchInput<'_>, id: &str) -> Status {
    let mut status = if input.ledger.assertions.contains_key(id) {
        input.ledger.status(id, input.now)
    } else {
        Status::Candidate
    };
    for transition in input
        .patch
        .transitions
        .iter()
        .filter(|t| t.assertion_id == id && t.recorded_at <= input.now)
    {
        status = match transition.action {
            Action::Assert => Status::Candidate,
            Action::Activate => Status::Active,
            Action::Dispute => Status::Disputed,
            Action::Supersede { .. } => Status::Superseded,
            Action::Retract => Status::Retracted,
            Action::Invalidate => Status::Invalidated,
            Action::Resolve => Status::Resolved,
        };
    }
    status
}

pub fn validate_versioned_patch(input: &VersionedPatchInput<'_>) -> Result<(), WorldError> {
    if !is_project_scope(input.project_scope) {
        return Err(WorldError::ScopeDenied);
    }
    let world_assertions: Vec<&Assertion> = input
        .patch
        .assertions
        .iter()
        .filter(|a| a.kind.is_world())
        .collect();
    if world_assertions.len() > MAX_NEW_WORLD_ASSERTIONS {
        return Err(WorldError::Limit);
    }
    for assertion in &world_assertions {
        if assertion.access.task_request.as_deref() != Some(input.project_scope) {
            return Err(WorldError::ScopeDenied);
        }
        if !assertion.access.purposes.contains(&Purpose::StateExtract)
            && !assertion.access.purposes.contains(&Purpose::Reasoning)
        {
            return Err(WorldError::ScopeDenied);
        }
    }
    // Every new world assertion needs a typed payload of the matching variant.
    let mut new_payloads: BTreeMap<&str, &VersionedWorldPayload> = BTreeMap::new();
    for assertion in &world_assertions {
        let payload = input
            .payloads
            .get(&assertion.payload_ref)
            .ok_or(WorldError::InvalidPayload)?;
        if !versioned_kind_matches(assertion.kind, payload) {
            return Err(WorldError::InvalidPayload);
        }
        new_payloads.insert(assertion.payload_ref.as_str(), payload);
        if assertion.semantic_key.len() > 128 {
            return Err(WorldError::InvalidPayload);
        }
    }
    for transition in &input.patch.transitions {
        if let Some(existing) = input.ledger.assertions.get(&transition.assertion_id) {
            if existing.kind.is_world() && !input.existing.contains_key(&existing.payload_ref) {
                return Err(WorldError::ProjectionCorrupt);
            }
        }
    }
    if world_assertions.is_empty() {
        return Ok(());
    }

    let new_refs = new_assertion_by_payload(&world_assertions);
    let entities = entity_index(input, &new_refs)?;

    for assertion in &world_assertions {
        let payload = new_payloads[assertion.payload_ref.as_str()];
        let view = payload.view(&assertion.semantic_key);
        match &view {
            WorldView::Entity(p) => validate_entity(input, assertion, &p.payload)?,
            WorldView::Relation(p) => validate_relation(input, assertion, &p.payload, &entities)?,
            WorldView::Focus(p) => validate_focus(input, assertion, &p.payload, &entities)?,
        }
        if expected_key(input.project_scope, assertion, payload)? != assertion.semantic_key {
            return Err(WorldError::InvalidPayload);
        }
    }

    enforce_identity(input, &world_assertions)?;
    enforce_capacity(input, &world_assertions)?;
    Ok(())
}

fn versioned_kind_matches(kind: Kind, payload: &VersionedWorldPayload) -> bool {
    matches!(
        (kind, payload),
        (
            Kind::WorldEntity,
            VersionedWorldPayload::V1(model::WorldPayload::Entity(_))
        ) | (
            Kind::WorldEntity,
            VersionedWorldPayload::V2(WorldPayloadV2::Entity(_))
        ) | (
            Kind::WorldRelation,
            VersionedWorldPayload::V1(model::WorldPayload::Relation(_))
        ) | (
            Kind::WorldRelation,
            VersionedWorldPayload::V2(WorldPayloadV2::Relation(_))
        ) | (
            Kind::WorldFocus,
            VersionedWorldPayload::V1(model::WorldPayload::Focus(_))
        ) | (
            Kind::WorldFocus,
            VersionedWorldPayload::V2(WorldPayloadV2::Focus(_))
        )
    )
}

fn validate_entity(
    input: &VersionedPatchInput<'_>,
    assertion: &Assertion,
    payload: &model_v2::EntityPayloadV2,
) -> Result<(), WorldError> {
    check_entity_v2_struct(payload).map_err(|_| WorldError::InvalidPayload)?;
    for dependency in &assertion.depends_on {
        if let Some(target) = input.ledger.assertions.get(dependency) {
            if target.kind.is_world() {
                return Err(WorldError::InvalidPayload);
            }
        }
    }
    if payload.entity_kind == EntityKindV2::Goal {
        let objective_id = payload
            .objective_assertion_id
            .as_deref()
            .ok_or(WorldError::InvalidReference)?;
        let objective = input
            .ledger
            .assertions
            .get(objective_id)
            .ok_or(WorldError::InvalidReference)?;
        if objective.kind != Kind::Objective
            || input.ledger.status(objective_id, input.now) != Status::Active
        {
            return Err(WorldError::InvalidReference);
        }
        if objective.access.task_request.as_deref() != Some(input.project_scope) {
            return Err(WorldError::ScopeDenied);
        }
        if !assertion.depends_on.contains(objective_id) {
            return Err(WorldError::InvalidReference);
        }
    }
    Ok(())
}

fn validate_relation(
    input: &VersionedPatchInput<'_>,
    assertion: &Assertion,
    payload: &RelationPayloadV2,
    entities: &BTreeMap<String, EntityRef>,
) -> Result<(), WorldError> {
    check_relation_v2_struct(payload).map_err(|e| match e.as_str() {
        "world-limit" => WorldError::Limit,
        _ => WorldError::InvalidPayload,
    })?;
    let from = entities
        .get(&payload.from_entity_id)
        .ok_or(WorldError::InvalidReference)?;
    let to = entities
        .get(&payload.to_entity_id)
        .ok_or(WorldError::InvalidReference)?;
    validate_endpoint_matrix(payload, from, to)?;
    validate_relation_evidence(input, assertion, payload)?;
    validate_relation_epistemic(input, assertion, payload)?;
    if from.assertion_id.is_empty() || to.assertion_id.is_empty() {
        return Err(WorldError::InvalidReference);
    }
    for endpoint in [from, to] {
        if !assertion.depends_on.contains(&endpoint.assertion_id) {
            return Err(WorldError::InvalidReference);
        }
    }
    Ok(())
}

fn validate_endpoint_matrix(
    payload: &RelationPayloadV2,
    from: &EntityRef,
    to: &EntityRef,
) -> Result<(), WorldError> {
    use model::EffectInput;
    match payload.relation_type {
        RelationTypeV2::Increases | RelationTypeV2::Decreases => {
            if to.kind != EntityKindV2::Metric {
                return Err(WorldError::InvalidPayload);
            }
            let expected = match from.kind {
                EntityKindV2::Metric => EffectInput::QuantityIncrease,
                EntityKindV2::Concept => EffectInput::Intervention,
                _ => return Err(WorldError::InvalidPayload),
            };
            if payload.effect_input != Some(expected) {
                return Err(WorldError::InvalidPayload);
            }
        }
        RelationTypeV2::Causes | RelationTypeV2::Enables | RelationTypeV2::Inhibits => {
            if from.kind != EntityKindV2::Concept || to.kind != EntityKindV2::Concept {
                return Err(WorldError::InvalidPayload);
            }
            if payload.effect_input != Some(EffectInput::Intervention) {
                return Err(WorldError::InvalidPayload);
            }
        }
        RelationTypeV2::HasGoal => {
            if !matches!(from.kind, EntityKindV2::Project | EntityKindV2::Actor)
                || to.kind != EntityKindV2::Goal
            {
                return Err(WorldError::InvalidPayload);
            }
        }
        RelationTypeV2::ServesGoal => {
            if to.kind != EntityKindV2::Goal {
                return Err(WorldError::InvalidPayload);
            }
            match from.kind {
                EntityKindV2::Metric => {
                    if payload.target_direction.is_none() {
                        return Err(WorldError::InvalidPayload);
                    }
                }
                EntityKindV2::Concept => {
                    if payload.target_direction.is_some() {
                        return Err(WorldError::InvalidPayload);
                    }
                }
                _ => return Err(WorldError::InvalidPayload),
            }
        }
        RelationTypeV2::CorrelatesWith => {
            if from.kind != EntityKindV2::Metric || to.kind != EntityKindV2::Metric {
                return Err(WorldError::InvalidPayload);
            }
        }
        RelationTypeV2::DependsOn => {
            if from.kind == EntityKindV2::Goal || to.kind == EntityKindV2::Goal {
                return Err(WorldError::InvalidPayload);
            }
        }
        RelationTypeV2::RelatedTo | RelationTypeV2::PartOf | RelationTypeV2::ImportantFor => {}
    }
    Ok(())
}

fn validate_relation_evidence(
    input: &VersionedPatchInput<'_>,
    assertion: &Assertion,
    payload: &RelationPayloadV2,
) -> Result<(), WorldError> {
    if payload.evidence_stances.len() > model::MAX_EVIDENCE_STANCES {
        return Err(WorldError::Limit);
    }
    let mut sources = BTreeSet::new();
    for stance in &payload.evidence_stances {
        if !assertion.evidence.contains(&stance.source) {
            return Err(WorldError::InvalidReference);
        }
        if !sources.insert(&stance.source) {
            return Err(WorldError::InvalidPayload);
        }
    }
    if payload.basis == model::Basis::UserStatement {
        let has_user_support = payload.evidence_stances.iter().any(|stance| {
            stance.stance == Stance::Supports
                && input
                    .ledger
                    .sources
                    .get(&stance.source)
                    .is_some_and(|s| s.role == SourceRole::User)
        });
        if !has_user_support {
            return Err(WorldError::InvalidPayload);
        }
    }
    // assessment_refs, prediction_source and outcome_source must be declared
    // inputs of the assertion.
    let declared: BTreeSet<&SourceKey> = assertion
        .evidence
        .iter()
        .chain(assertion.input_dependencies.iter())
        .collect();
    for source in &payload.assessment_refs {
        if !declared.contains(source) {
            return Err(WorldError::InvalidReference);
        }
    }
    if let Some(update) = &payload.outcome_update {
        if !declared.contains(&update.prediction_source)
            || !declared.contains(&update.outcome_source)
        {
            return Err(WorldError::InvalidReference);
        }
    }
    if let Some(confidence) = &payload.confidence {
        match confidence.method {
            model_v2::ConfidenceMethod::ManualV1 => {}
            model_v2::ConfidenceMethod::CounterevidenceV1 => {
                validate_counterevidence(input, assertion, payload)?;
            }
            model_v2::ConfidenceMethod::PathRankV1 => {
                return Err(WorldError::InvalidPayload);
            }
        }
    }
    Ok(())
}

fn validate_counterevidence(
    input: &VersionedPatchInput<'_>,
    assertion: &Assertion,
    payload: &RelationPayloadV2,
) -> Result<(), WorldError> {
    let update = payload
        .outcome_update
        .as_ref()
        .ok_or(WorldError::InvalidPayload)?;
    let prior_id = &update.prior_assertion_id;
    let prior = input
        .ledger
        .assertions
        .get(prior_id)
        .ok_or(WorldError::InvalidReference)?;
    let Some(prior_payload) = input.existing.get(&prior.payload_ref) else {
        return Err(WorldError::ProjectionCorrupt);
    };
    let WorldView::Relation(prior_view) = view_payload(prior_payload, &prior.semantic_key) else {
        return Err(WorldError::InvalidPayload);
    };
    // The new relation must be the same relation, now disputed.
    if payload.epistemic != Epistemic::Disputed {
        return Err(WorldError::InvalidPayload);
    }
    if identity_v2::relation_identity_from_payload(
        input.project_scope,
        &prior_view.payload,
        prior.valid_from,
        prior.valid_until,
    ) != identity_v2::relation_identity_from_payload(
        input.project_scope,
        payload,
        assertion.valid_from,
        assertion.valid_until,
    ) {
        return Err(WorldError::InvalidPayload);
    }
    // Recompute the comparison from the stored update; the stored expected /
    // actual directions must themselves form the counterexample.
    let conditions = identity_v2::sorted_conditions(&prior_view.payload);
    let prediction = super::outcome_v2::Prediction {
        metric_id: prior_view.payload.to_entity_id.clone(),
        comparison_id: prior_view.payload.comparison_id.clone(),
        conditions: conditions.clone(),
        direction: update.expected,
        at_ms: update.predicted_at_ms,
        source: update.prediction_source.clone(),
    };
    let outcome = super::outcome_v2::Outcome {
        metric_id: prior_view.payload.to_entity_id.clone(),
        comparison_id: prior_view.payload.comparison_id.clone(),
        conditions,
        direction: update.actual,
        at_ms: update.observed_at_ms,
        source: update.outcome_source.clone(),
    };
    let recomputed = super::outcome_v2::compare_outcome(
        &prior_view.payload,
        prior.valid_from,
        prior.valid_until,
        prior_view.payload.confidence.as_ref(),
        &prediction,
        &outcome,
    )?;
    if !recomputed.disputed {
        return Err(WorldError::InvalidPayload);
    }
    if payload.confidence != recomputed.confidence {
        return Err(WorldError::InvalidPayload);
    }
    Ok(())
}

fn validate_relation_epistemic(
    input: &VersionedPatchInput<'_>,
    assertion: &Assertion,
    payload: &RelationPayloadV2,
) -> Result<(), WorldError> {
    let status = planned_status(input, &assertion.id);
    let is_signed = matches!(
        payload.relation_type,
        RelationTypeV2::Increases
            | RelationTypeV2::Decreases
            | RelationTypeV2::Causes
            | RelationTypeV2::Enables
            | RelationTypeV2::Inhibits
    );
    if is_signed && payload.outcome_update.is_none() && payload.epistemic != Epistemic::Hypothesis {
        return Err(WorldError::InvalidPayload);
    }
    if payload.epistemic == Epistemic::Disputed && status != Status::Disputed {
        return Err(WorldError::InvalidPayload);
    }
    if payload.epistemic != Epistemic::Disputed && status == Status::Disputed {
        return Err(WorldError::InvalidPayload);
    }
    if let Some(strength) = &payload.strength {
        if strength.magnitude > 1000 {
            return Err(WorldError::InvalidPayload);
        }
    }
    Ok(())
}

fn validate_focus(
    input: &VersionedPatchInput<'_>,
    assertion: &Assertion,
    payload: &model_v2::FocusPayloadV2,
    entities: &BTreeMap<String, EntityRef>,
) -> Result<(), WorldError> {
    check_focus_v2_struct(payload).map_err(|_| WorldError::InvalidPayload)?;
    let entity = entities
        .get(&payload.entity_id)
        .ok_or(WorldError::InvalidReference)?;
    if entity.assertion_id.is_empty() || !assertion.depends_on.contains(&entity.assertion_id) {
        return Err(WorldError::InvalidReference);
    }
    if payload.reason == FocusReason::CurrentWork {
        let objective_id = payload
            .objective_assertion_id
            .as_deref()
            .ok_or(WorldError::InvalidReference)?;
        let objective = input
            .ledger
            .assertions
            .get(objective_id)
            .ok_or(WorldError::InvalidReference)?;
        if objective.kind != Kind::Objective
            || input.ledger.status(objective_id, input.now) != Status::Active
        {
            return Err(WorldError::InvalidReference);
        }
        if objective.access.task_request.as_deref() != Some(input.project_scope) {
            return Err(WorldError::ScopeDenied);
        }
        if !assertion.depends_on.contains(objective_id) {
            return Err(WorldError::InvalidReference);
        }
    }
    Ok(())
}

/// Version-aware expected semantic key. v1 keeps its `wm1:` shape; v2 uses the
/// fixed C2 arrays. Running normalized v1 payloads through the v2 semantic
/// checks above is safe because the six v1 relation types are a subset.
fn expected_key(
    project_scope: &str,
    assertion: &Assertion,
    payload: &VersionedWorldPayload,
) -> Result<String, WorldError> {
    match payload {
        VersionedWorldPayload::V1(model::WorldPayload::Entity(p)) => {
            Ok(identity::entity_key(project_scope, &p.entity_id))
        }
        VersionedWorldPayload::V1(model::WorldPayload::Relation(p)) => {
            let conditions = model::check_conditions_struct(&p.conditions)
                .map_err(|_| WorldError::InvalidPayload)?;
            let (from, to) = if p.relation_type == model::RelationType::RelatedTo {
                identity::order_undirected(&p.from_entity_id, &p.to_entity_id)
            } else {
                (p.from_entity_id.clone(), p.to_entity_id.clone())
            };
            Ok(identity::relation_key(&RelationKeyInput {
                project_scope,
                from: &from,
                to: &to,
                relation_type: p.relation_type.as_str(),
                effect_input: p.effect_input.map(|e| e.as_str()),
                sorted_conditions: &conditions,
                valid_from: assertion.valid_from,
                valid_until: assertion.valid_until,
            }))
        }
        VersionedWorldPayload::V1(model::WorldPayload::Focus(p)) => Ok(identity::focus_key(
            project_scope,
            &p.entity_id,
            match p.reason {
                FocusReason::CurrentWork => "current_work",
                FocusReason::ExplicitInterest => "explicit_interest",
            },
        )),
        VersionedWorldPayload::V2(WorldPayloadV2::Entity(p)) => {
            Ok(identity_v2::entity_key_v2(project_scope, &p.entity_id))
        }
        VersionedWorldPayload::V2(WorldPayloadV2::Relation(p)) => {
            Ok(identity_v2::relation_key_from_payload(
                project_scope,
                p,
                assertion.valid_from,
                assertion.valid_until,
            ))
        }
        VersionedWorldPayload::V2(WorldPayloadV2::Focus(p)) => Ok(identity_v2::focus_key_v2(
            project_scope,
            &p.entity_id,
            match p.reason {
                FocusReason::CurrentWork => "current_work",
                FocusReason::ExplicitInterest => "explicit_interest",
            },
        )),
    }
}

fn kind_code(kind: Kind) -> &'static str {
    match kind {
        Kind::WorldEntity => "world_entity",
        Kind::WorldRelation => "world_relation",
        Kind::WorldFocus => "world_focus",
        _ => "other",
    }
}

/// Cross-version duplicate identity (C2). No two live World assertions in the
/// same project may share the same logical identity array.
fn enforce_identity(input: &VersionedPatchInput<'_>, new: &[&Assertion]) -> Result<(), WorldError> {
    let mut live: BTreeMap<(&'static str, String), Vec<(String, i64)>> = BTreeMap::new();
    let mut record = |kind: Kind, identity: serde_json::Value, id: &str, version: i64| {
        live.entry((kind_code(kind), identity.to_string()))
            .or_default()
            .push((id.to_string(), version));
    };
    for (id, assertion) in &input.ledger.assertions {
        if !assertion.kind.is_world() {
            continue;
        }
        if planned_status(input, id) != Status::Active {
            continue;
        }
        let Some(payload) = input.existing.get(&assertion.payload_ref) else {
            return Err(WorldError::ProjectionCorrupt);
        };
        // Identity includes the project scope, so every assertion is keyed with
        // its own scope. This keeps projects isolated.
        let scope = assertion
            .access
            .task_request
            .as_deref()
            .unwrap_or(input.project_scope);
        if let Some(identity) =
            logical_identity(scope, payload, assertion.valid_from, assertion.valid_until)
        {
            record(assertion.kind, identity, id, payload.schema_version());
        }
    }
    for assertion in new {
        if planned_status(input, &assertion.id) != Status::Active {
            continue;
        }
        let payload = input
            .payloads
            .get(&assertion.payload_ref)
            .ok_or(WorldError::InvalidPayload)?;
        // New assertions are already constrained to `input.project_scope`.
        if let Some(identity) = logical_identity(
            input.project_scope,
            payload,
            assertion.valid_from,
            assertion.valid_until,
        ) {
            record(
                assertion.kind,
                identity,
                &assertion.id,
                payload.schema_version(),
            );
        }
    }
    // A pure-v1 duplicate is left to the reducer's existing Conflict path so v1
    // behavior is unchanged. Any v2 or cross-version duplicate is the new,
    // prefix-insensitive `world-duplicate-identity`.
    if live
        .values()
        .any(|entries| entries.len() > 1 && entries.iter().any(|(_, version)| *version == 2))
    {
        return Err(WorldError::DuplicateIdentity);
    }
    Ok(())
}

fn logical_identity(
    project_scope: &str,
    payload: &VersionedWorldPayload,
    valid_from: i64,
    valid_until: Option<i64>,
) -> Option<serde_json::Value> {
    match view_payload(payload, "") {
        WorldView::Entity(view) => Some(identity_v2::entity_identity_from_payload(
            project_scope,
            &view.payload,
        )),
        WorldView::Relation(view) => Some(identity_v2::relation_identity_from_payload(
            project_scope,
            &view.payload,
            valid_from,
            valid_until,
        )),
        WorldView::Focus(view) => Some(identity_v2::focus_identity_from_payload(
            project_scope,
            &view.payload,
        )),
    }
}

fn enforce_capacity(input: &VersionedPatchInput<'_>, new: &[&Assertion]) -> Result<(), WorldError> {
    let freed: BTreeSet<&str> = input
        .patch
        .transitions
        .iter()
        .filter_map(|t| match &t.action {
            Action::Supersede { .. } | Action::Retract | Action::Invalidate => {
                Some(t.assertion_id.as_str())
            }
            _ => None,
        })
        .collect();
    let existing = input
        .ledger
        .assertions
        .iter()
        .filter(|(id, a)| {
            a.kind.is_world()
                && !freed.contains(id.as_str())
                && matches!(
                    input.ledger.status(id, input.now),
                    Status::Candidate | Status::Active | Status::Disputed
                )
        })
        .count();
    if existing + new.len() > MAX_WORLD_ASSERTIONS {
        return Err(WorldError::Limit);
    }
    Ok(())
}
