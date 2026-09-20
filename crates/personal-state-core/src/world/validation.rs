//! World patch validation (WM-03/WM-04/WM-07). Pure: no DB, clock or ID.
//!
//! The adapter decodes existing World payloads and passes them in as typed
//! values, so this module never reads the erasable payload store itself.

use crate::world::model::{
    self, EntityPayload, FocusPayload, FocusReason, RelationPayload, WorldPayload,
};
use crate::world::{identity, RelationKeyInput};
use crate::*;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_NEW_WORLD_ASSERTIONS: usize = 8;
pub const MAX_WORLD_ASSERTIONS: usize = 10_000;
pub const MAX_WORLD_SOURCES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldError {
    InvalidPayload,
    InvalidReference,
    ScopeDenied,
    Limit,
    BudgetTooSmall,
    ProjectionCorrupt,
    DuplicateIdentity,
}

impl WorldError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidPayload => "world-invalid-payload",
            Self::InvalidReference => "world-invalid-reference",
            Self::ScopeDenied => "world-scope-denied",
            Self::Limit => "world-limit",
            Self::BudgetTooSmall => "world-budget-too-small",
            Self::ProjectionCorrupt => "world-projection-corrupt",
            Self::DuplicateIdentity => "world-duplicate-identity",
        }
    }
}

impl std::fmt::Display for WorldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for WorldError {}

/// Trusted validation inputs. `project_scope` is the adapter-confirmed
/// `project:<opaque-id>`; it is never taken from model text.
pub struct WorldPatchInput<'a> {
    pub ledger: &'a Ledger,
    pub patch: &'a StatePatch,
    /// New typed payloads keyed by `Assertion.payload_ref`.
    pub payloads: &'a BTreeMap<String, WorldPayload>,
    /// Existing typed World payloads keyed by `Assertion.payload_ref`, decoded
    /// by the adapter from the erasable payload store.
    pub existing: &'a BTreeMap<String, WorldPayload>,
    pub project_scope: &'a str,
    pub now: i64,
}

fn is_project_scope(value: &str) -> bool {
    value
        .strip_prefix("project:")
        .is_some_and(|id| !id.is_empty())
}

/// Entity identity resolved inside one project scope.
struct EntityRef {
    assertion_id: String,
    kind: model::EntityKind,
}

fn new_assertion_by_payload<'a>(world: &'a [&'a Assertion]) -> BTreeMap<&'a str, &'a str> {
    world
        .iter()
        .map(|a| (a.payload_ref.as_str(), a.id.as_str()))
        .collect()
}

fn entity_index(
    input: &WorldPatchInput<'_>,
    new: &BTreeMap<&str, &WorldPayload>,
    new_assertion: &BTreeMap<&str, &str>,
) -> Result<BTreeMap<String, EntityRef>, WorldError> {
    let mut index: BTreeMap<String, EntityRef> = BTreeMap::new();
    // Only currently Active entities may be referenced: the reducer's Activate
    // requires every `depends_on` target to be Active, so a Candidate or
    // Disputed endpoint must fail validation here rather than at apply time.
    for (id, assertion) in &input.ledger.assertions {
        if assertion.kind != Kind::WorldEntity
            || !assertion.access.permits_scope(input.project_scope)
        {
            continue;
        }
        if input.ledger.status(id, input.now) != Status::Active {
            continue;
        }
        let Some(WorldPayload::Entity(payload)) = input.existing.get(&assertion.payload_ref) else {
            return Err(WorldError::ProjectionCorrupt);
        };
        // First Active assertion wins deterministically (ids sort in BTreeMap order).
        index.entry(payload.entity_id.clone()).or_insert(EntityRef {
            assertion_id: id.clone(),
            kind: payload.entity_kind,
        });
    }
    for (payload_ref, payload) in new {
        if let WorldPayload::Entity(p) = *payload {
            let assertion_id = new_assertion
                .get(*payload_ref)
                .map(|id| (*id).to_string())
                .unwrap_or_default();
            index.insert(
                p.entity_id.clone(),
                EntityRef {
                    assertion_id,
                    kind: p.entity_kind,
                },
            );
        }
    }
    Ok(index)
}

pub fn validate_world_patch(input: &WorldPatchInput<'_>) -> Result<(), WorldError> {
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
    let mut new_payloads: BTreeMap<&str, &WorldPayload> = BTreeMap::new();
    for assertion in &world_assertions {
        let payload = input
            .payloads
            .get(&assertion.payload_ref)
            .ok_or(WorldError::InvalidPayload)?;
        if !matches!(
            (assertion.kind, payload),
            (Kind::WorldEntity, WorldPayload::Entity(_))
                | (Kind::WorldRelation, WorldPayload::Relation(_))
                | (Kind::WorldFocus, WorldPayload::Focus(_))
        ) {
            return Err(WorldError::InvalidPayload);
        }
        new_payloads.insert(assertion.payload_ref.as_str(), payload);
    }
    // A transition that targets an existing World assertion must have a typed
    // payload available for re-validation.
    for transition in &input.patch.transitions {
        if let Some(existing) = input.ledger.assertions.get(&transition.assertion_id) {
            if existing.kind.is_world() && !input.existing.contains_key(&existing.payload_ref) {
                return Err(WorldError::ProjectionCorrupt);
            }
        }
    }
    if new_payloads.is_empty() {
        return Ok(());
    }

    let entities = entity_index(
        input,
        &new_payloads,
        &new_assertion_by_payload(&world_assertions),
    )?;

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

    for (payload_ref, payload) in &new_payloads {
        let assertion = world_assertions
            .iter()
            .find(|a| a.payload_ref == *payload_ref)
            .expect("payload belongs to a patch assertion");
        match payload {
            WorldPayload::Entity(p) => validate_entity(assertion, p, input)?,
            WorldPayload::Relation(p) => validate_relation(assertion, p, input, &entities)?,
            WorldPayload::Focus(p) => validate_focus(assertion, p, input, &entities)?,
        }
        if assertion.semantic_key.len() > 128 {
            return Err(WorldError::InvalidPayload);
        }
    }

    enforce_capacity(input, &world_assertions)?;
    Ok(())
}

fn validate_entity(
    assertion: &Assertion,
    payload: &EntityPayload,
    input: &WorldPatchInput<'_>,
) -> Result<(), WorldError> {
    model::check_entity_struct(payload).map_err(|_| WorldError::InvalidPayload)?;
    if payload.schema_version != model::WORLD_SCHEMA_VERSION {
        return Err(WorldError::InvalidPayload);
    }
    let expected = identity::entity_key(input.project_scope, &payload.entity_id);
    if assertion.semantic_key != expected {
        return Err(WorldError::InvalidPayload);
    }
    // World relations must never be written as assertion graph dependencies.
    for dependency in &assertion.depends_on {
        if let Some(target) = input.ledger.assertions.get(dependency) {
            if target.kind.is_world() {
                return Err(WorldError::InvalidPayload);
            }
        }
    }
    Ok(())
}

fn validate_relation(
    assertion: &Assertion,
    payload: &RelationPayload,
    input: &WorldPatchInput<'_>,
    entities: &BTreeMap<String, EntityRef>,
) -> Result<(), WorldError> {
    let conditions =
        model::check_relation_struct(payload).map_err(|_| WorldError::InvalidPayload)?;
    let from = entities
        .get(&payload.from_entity_id)
        .ok_or(WorldError::InvalidReference)?;
    let to = entities
        .get(&payload.to_entity_id)
        .ok_or(WorldError::InvalidReference)?;
    match payload.relation_type {
        model::RelationType::Increases | model::RelationType::Decreases => {
            if to.kind != model::EntityKind::Metric {
                return Err(WorldError::InvalidPayload);
            }
            let expected_effect = match from.kind {
                model::EntityKind::Metric => model::EffectInput::QuantityIncrease,
                model::EntityKind::Concept => model::EffectInput::Intervention,
                // A project is a work context, not a causal input quantity.
                model::EntityKind::Project => return Err(WorldError::InvalidPayload),
            };
            if payload.effect_input != Some(expected_effect) {
                return Err(WorldError::InvalidPayload);
            }
        }
        _ => {
            if payload.effect_input.is_some() {
                return Err(WorldError::InvalidPayload);
            }
        }
    }
    validate_basis(assertion, payload, input)?;
    if from.assertion_id.is_empty() || to.assertion_id.is_empty() {
        return Err(WorldError::InvalidReference);
    }
    for endpoint in [from, to] {
        if !assertion.depends_on.contains(&endpoint.assertion_id) {
            return Err(WorldError::InvalidReference);
        }
    }
    let (from_id, to_id) = if payload.relation_type == model::RelationType::RelatedTo {
        identity::order_undirected(&payload.from_entity_id, &payload.to_entity_id)
    } else {
        (payload.from_entity_id.clone(), payload.to_entity_id.clone())
    };
    let expected = identity::relation_key(&RelationKeyInput {
        project_scope: input.project_scope,
        from: &from_id,
        to: &to_id,
        relation_type: payload.relation_type.as_str(),
        effect_input: payload.effect_input.map(|e| e.as_str()),
        sorted_conditions: &conditions,
        valid_from: assertion.valid_from,
        valid_until: assertion.valid_until,
    });
    if assertion.semantic_key != expected {
        return Err(WorldError::InvalidPayload);
    }
    Ok(())
}

fn validate_basis(
    assertion: &Assertion,
    payload: &RelationPayload,
    input: &WorldPatchInput<'_>,
) -> Result<(), WorldError> {
    if payload.evidence_stances.len() > model::MAX_EVIDENCE_STANCES {
        return Err(WorldError::Limit);
    }
    let mut sources = BTreeSet::new();
    for stance in &payload.evidence_stances {
        if !assertion.evidence.contains(&stance.source) {
            return Err(WorldError::InvalidReference);
        }
        if !sources.insert(stance.source.clone()) {
            return Err(WorldError::InvalidPayload);
        }
    }
    if payload.basis == model::Basis::UserStatement {
        let has_user_support = payload.evidence_stances.iter().any(|stance| {
            stance.stance == model::Stance::Supports
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
    Ok(())
}

fn validate_focus(
    assertion: &Assertion,
    payload: &FocusPayload,
    input: &WorldPatchInput<'_>,
    entities: &BTreeMap<String, EntityRef>,
) -> Result<(), WorldError> {
    model::check_focus_struct(payload).map_err(|_| WorldError::InvalidPayload)?;
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
    let expected = identity::focus_key(
        input.project_scope,
        &payload.entity_id,
        match payload.reason {
            FocusReason::CurrentWork => "current_work",
            FocusReason::ExplicitInterest => "explicit_interest",
        },
    );
    if assertion.semantic_key != expected {
        return Err(WorldError::InvalidPayload);
    }
    Ok(())
}

/// Capacity counts Candidate/Active/Disputed World assertions after subtracting
/// items replaced, retracted or invalidated by this same patch. Retract and
/// forget are never blocked by the cap.
fn enforce_capacity(input: &WorldPatchInput<'_>, new: &[&Assertion]) -> Result<(), WorldError> {
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

trait ScopePermits {
    fn permits_scope(&self, scope: &str) -> bool;
}
impl ScopePermits for AccessScope {
    fn permits_scope(&self, scope: &str) -> bool {
        self.task_request.as_deref() == Some(scope)
    }
}
