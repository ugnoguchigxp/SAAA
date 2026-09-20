//! Pure three-valued condition and availability evaluation (D14/D15).
//!
//! Observations are already checked for scope, source validity and time by the
//! adapter; this module only does the deterministic aggregation. It never
//! creates or updates a node from an observation.

use super::model::Condition;
use super::slice_v2::{AvailabilityStateV2, ConditionStateV2};
use crate::SourceKey;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionObservation {
    pub relation_assertion_id: String,
    pub key: String,
    pub value: String,
    pub evidence: SourceKey,
    pub valid_from_ms: i64,
    pub valid_until_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvailabilityValue {
    Available,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailabilityObservation {
    pub entity_id: String,
    pub value: AvailabilityValue,
    pub evidence: SourceKey,
    pub valid_from_ms: i64,
    pub valid_until_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionResult {
    pub relation_assertion_id: String,
    pub key: String,
    pub expected_value: String,
    pub state: ConditionStateV2,
    pub evidence: Vec<SourceKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionEvaluation {
    pub items: Vec<ConditionResult>,
    pub aggregate: ConditionStateV2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailabilityEvaluation {
    pub state: AvailabilityStateV2,
    pub evidence: Vec<SourceKey>,
}

/// Evaluate each stored condition for one relation. Observations are matched by
/// `relation_assertion_id` first, so the same `key` name never leaks between
/// relations.
pub fn evaluate_conditions(
    relation_assertion_id: &str,
    conditions: &[Condition],
    observations: &[ConditionObservation],
) -> ConditionEvaluation {
    let mut items = Vec::with_capacity(conditions.len());
    let mut aggregate = ConditionStateV2::Unknown;
    if conditions.is_empty() {
        return ConditionEvaluation { items, aggregate };
    }
    aggregate = ConditionStateV2::Satisfied;
    for condition in conditions {
        let matching: Vec<&ConditionObservation> = observations
            .iter()
            .filter(|o| o.relation_assertion_id == relation_assertion_id && o.key == condition.key)
            .collect();
        let mut values: BTreeSet<&str> = BTreeSet::new();
        let mut evidence: BTreeSet<SourceKey> = BTreeSet::new();
        for observation in &matching {
            values.insert(observation.value.as_str());
            evidence.insert(observation.evidence.clone());
        }
        let state = if values.len() == 1 {
            if values.contains(condition.value.as_str()) {
                ConditionStateV2::Satisfied
            } else {
                ConditionStateV2::Violated
            }
        } else {
            // Zero observations or a conflict is unknown.
            ConditionStateV2::Unknown
        };
        match state {
            ConditionStateV2::Violated => aggregate = ConditionStateV2::Violated,
            ConditionStateV2::Unknown if aggregate == ConditionStateV2::Satisfied => {
                aggregate = ConditionStateV2::Unknown;
            }
            _ => {}
        }
        items.push(ConditionResult {
            relation_assertion_id: relation_assertion_id.to_string(),
            key: condition.key.clone(),
            expected_value: condition.value.clone(),
            state,
            evidence: evidence.into_iter().collect(),
        });
    }
    ConditionEvaluation { items, aggregate }
}

/// Availability is `unknown` with no observation or with conflicting ones. A
/// node existing in the graph is never by itself "available".
pub fn evaluate_availability(
    entity_id: &str,
    observations: &[AvailabilityObservation],
) -> AvailabilityEvaluation {
    let mut values: BTreeSet<bool> = BTreeSet::new();
    let mut evidence: BTreeSet<SourceKey> = BTreeSet::new();
    for observation in observations.iter().filter(|o| o.entity_id == entity_id) {
        values.insert(matches!(observation.value, AvailabilityValue::Available));
        evidence.insert(observation.evidence.clone());
    }
    let state = if values.len() == 1 {
        if values.contains(&true) {
            AvailabilityStateV2::Available
        } else {
            AvailabilityStateV2::Unavailable
        }
    } else {
        AvailabilityStateV2::Unknown
    };
    AvailabilityEvaluation {
        state,
        evidence: evidence.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> SourceKey {
        SourceKey {
            id: id.into(),
            version: 1,
            start: 0,
            end: 1,
        }
    }

    fn observation(relation: &str, value: &str, evidence: &str) -> ConditionObservation {
        ConditionObservation {
            relation_assertion_id: relation.into(),
            key: "config".into(),
            value: value.into(),
            evidence: key(evidence),
            valid_from_ms: 0,
            valid_until_ms: Some(200),
        }
    }

    fn condition() -> Vec<Condition> {
        vec![Condition {
            key: "config".into(),
            value: "a".into(),
        }]
    }

    #[test]
    fn d14_single_match_is_satisfied_and_mismatch_violated() {
        let satisfied = evaluate_conditions("r1", &condition(), &[observation("r1", "a", "s1")]);
        assert_eq!(satisfied.aggregate, ConditionStateV2::Satisfied);
        let violated = evaluate_conditions("r1", &condition(), &[observation("r1", "b", "s1")]);
        assert_eq!(violated.aggregate, ConditionStateV2::Violated);
    }

    #[test]
    fn d14_conflict_and_other_relation_are_unknown() {
        let conflict = evaluate_conditions(
            "r1",
            &condition(),
            &[observation("r1", "a", "s1"), observation("r1", "b", "s2")],
        );
        assert_eq!(conflict.aggregate, ConditionStateV2::Unknown);
        let other = evaluate_conditions("r1", &condition(), &[observation("r2", "a", "s1")]);
        assert_eq!(other.aggregate, ConditionStateV2::Unknown);
    }

    #[test]
    fn d14_empty_conditions_are_unknown() {
        let empty = evaluate_conditions("r1", &[], &[observation("r1", "a", "s1")]);
        assert_eq!(empty.aggregate, ConditionStateV2::Unknown);
        assert!(empty.items.is_empty());
    }

    #[test]
    fn d15_availability_three_states() {
        let available = AvailabilityObservation {
            entity_id: "b".into(),
            value: AvailabilityValue::Available,
            evidence: key("s1"),
            valid_from_ms: 0,
            valid_until_ms: None,
        };
        assert_eq!(
            evaluate_availability("b", std::slice::from_ref(&available)).state,
            AvailabilityStateV2::Available
        );
        let mut unavailable = available.clone();
        unavailable.value = AvailabilityValue::Unavailable;
        assert_eq!(
            evaluate_availability("b", &[available.clone(), unavailable]).state,
            AvailabilityStateV2::Unknown
        );
        assert_eq!(
            evaluate_availability("b", &[]).state,
            AvailabilityStateV2::Unknown
        );
    }
}
