//! Prediction / Outcome comparison and counterevidence decay (D34).
//! Pure functions: no DB, no saving, no clock.

use super::model::Condition;
use super::model_v2::{
    Confidence, ConfidenceMethod, EffectDirection, Epistemic, RelationPayloadV2, RelationTypeV2,
};
use super::validation::WorldError;
use crate::SourceKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prediction {
    pub metric_id: String,
    pub comparison_id: Option<String>,
    pub conditions: Vec<(String, String)>,
    pub direction: EffectDirection,
    pub at_ms: i64,
    pub source: SourceKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub metric_id: String,
    pub comparison_id: Option<String>,
    pub conditions: Vec<(String, String)>,
    pub direction: EffectDirection,
    pub at_ms: i64,
    pub source: SourceKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeVerdict {
    /// Not the same comparison basis or time window: no evidence either way.
    MissingCondition,
    /// Same direction or undecidable direction.
    NoChange,
    /// The observed direction contradicts the predicted direction.
    Counterexample,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeComparison {
    pub verdict: OutcomeVerdict,
    pub disputed: bool,
    /// Recomputed `counterevidence_v1` confidence, or `None` when the prior
    /// relation had no evaluated confidence.
    pub confidence: Option<Confidence>,
}

fn sorted_conditions(conditions: &[Condition]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = conditions
        .iter()
        .map(|c| (c.key.clone(), c.value.clone()))
        .collect();
    pairs.sort();
    pairs
}

fn directions_oppose(expected: EffectDirection, actual: EffectDirection) -> bool {
    matches!(
        (expected, actual),
        (EffectDirection::Increase, EffectDirection::Decrease)
            | (EffectDirection::Decrease, EffectDirection::Increase)
    )
}

fn decidable(direction: EffectDirection) -> bool {
    !matches!(
        direction,
        EffectDirection::Unknown | EffectDirection::Unchanged | EffectDirection::Mixed
    )
}

/// `floor(old * 8 / 10)`, computed on the stored thousandths value.
pub fn counterevidence_score(prior: &Confidence) -> u16 {
    ((prior.value as u32) * 8 / 10) as u16
}

/// Compare a stored prediction with an observed outcome against the prior
/// relation version. `MissingCondition` never changes the score.
pub fn compare_outcome(
    prior: &RelationPayloadV2,
    prior_valid_from: i64,
    prior_valid_until: Option<i64>,
    prior_confidence: Option<&Confidence>,
    prediction: &Prediction,
    outcome: &Outcome,
) -> Result<OutcomeComparison, WorldError> {
    if !matches!(
        prior.relation_type,
        RelationTypeV2::Increases | RelationTypeV2::Decreases
    ) {
        return Err(WorldError::InvalidPayload);
    }
    let relation_conditions = sorted_conditions(&prior.conditions);
    let comparable = prior.comparison_id.is_some()
        && prediction.comparison_id == prior.comparison_id
        && outcome.comparison_id == prior.comparison_id
        && !relation_conditions.is_empty()
        && prediction.conditions == relation_conditions
        && outcome.conditions == relation_conditions
        && outcome.metric_id == prior.to_entity_id
        && prediction.metric_id == prior.to_entity_id
        && outcome.at_ms >= prediction.at_ms
        && outcome.at_ms >= prior_valid_from
        && prior_valid_until.is_none_or(|until| outcome.at_ms < until);
    if !comparable {
        return Ok(OutcomeComparison {
            verdict: OutcomeVerdict::MissingCondition,
            disputed: false,
            confidence: prior_confidence.cloned(),
        });
    }
    if !decidable(prediction.direction) || !decidable(outcome.direction) {
        return Ok(OutcomeComparison {
            verdict: OutcomeVerdict::NoChange,
            disputed: false,
            confidence: prior_confidence.cloned(),
        });
    }
    if directions_oppose(prediction.direction, outcome.direction) {
        return Ok(OutcomeComparison {
            verdict: OutcomeVerdict::Counterexample,
            disputed: true,
            confidence: prior_confidence
                .map(counterevidence_score)
                .map(|value| Confidence {
                    value,
                    method: ConfidenceMethod::CounterevidenceV1,
                }),
        });
    }
    Ok(OutcomeComparison {
        verdict: OutcomeVerdict::NoChange,
        disputed: false,
        confidence: prior_confidence.cloned(),
    })
}

/// Convenience for the F scenario: build the disputed copy of a relation.
pub fn disputed_copy(prior: &RelationPayloadV2, confidence_value: u16) -> RelationPayloadV2 {
    let mut next = prior.clone();
    next.epistemic = Epistemic::Disputed;
    next.confidence = Some(Confidence {
        value: confidence_value,
        method: ConfidenceMethod::CounterevidenceV1,
    });
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::model::{Basis, Condition, EffectInput, EvidenceStance, RelationTag, Stance};
    use crate::world::model_v2::{MechanismState, RelationPayloadV2};

    fn key() -> SourceKey {
        SourceKey {
            id: "s1".into(),
            version: 1,
            start: 0,
            end: 1,
        }
    }

    fn relation() -> RelationPayloadV2 {
        RelationPayloadV2 {
            payload_type: RelationTag::Relation,
            schema_version: 2,
            from_entity_id: "a".into(),
            to_entity_id: "m".into(),
            relation_type: RelationTypeV2::Decreases,
            effect_input: Some(EffectInput::Intervention),
            conditions: vec![Condition {
                key: "config".into(),
                value: "a".into(),
            }],
            comparison_id: Some("cmp".into()),
            basis: Basis::ModelHypothesis,
            evidence_stances: vec![EvidenceStance {
                source: key(),
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
            assessment_refs: vec![key()],
            mechanism: MechanismState::Unassessed,
            outcome_update: None,
        }
    }

    fn prediction(direction: EffectDirection) -> Prediction {
        Prediction {
            metric_id: "m".into(),
            comparison_id: Some("cmp".into()),
            conditions: vec![("config".into(), "a".into())],
            direction,
            at_ms: 10,
            source: key(),
        }
    }

    fn outcome(direction: EffectDirection) -> Outcome {
        Outcome {
            metric_id: "m".into(),
            comparison_id: Some("cmp".into()),
            conditions: vec![("config".into(), "a".into())],
            direction,
            at_ms: 20,
            source: key(),
        }
    }

    #[test]
    fn d34_counterexample_decays_score() {
        let relation = relation();
        let result = compare_outcome(
            &relation,
            0,
            Some(100),
            relation.confidence.as_ref(),
            &prediction(EffectDirection::Decrease),
            &outcome(EffectDirection::Increase),
        )
        .unwrap();
        assert_eq!(result.verdict, OutcomeVerdict::Counterexample);
        assert!(result.disputed);
        assert_eq!(result.confidence.unwrap().value, 640);
    }

    #[test]
    fn d34_seven_nine_nine_decays_to_six_three_nine() {
        let mut relation = relation();
        relation.confidence = Some(Confidence {
            value: 799,
            method: ConfidenceMethod::ManualV1,
        });
        let result = compare_outcome(
            &relation,
            0,
            Some(100),
            relation.confidence.as_ref(),
            &prediction(EffectDirection::Decrease),
            &outcome(EffectDirection::Increase),
        )
        .unwrap();
        assert_eq!(result.confidence.unwrap().value, 639);
    }

    #[test]
    fn d34_missing_condition_keeps_score() {
        let relation = relation();
        let mut other = outcome(EffectDirection::Increase);
        other.conditions = vec![("config".into(), "b".into())];
        let result = compare_outcome(
            &relation,
            0,
            Some(100),
            relation.confidence.as_ref(),
            &prediction(EffectDirection::Decrease),
            &other,
        )
        .unwrap();
        assert_eq!(result.verdict, OutcomeVerdict::MissingCondition);
        assert_eq!(result.confidence.unwrap().value, 800);
    }

    #[test]
    fn d34_unknown_direction_does_not_decay() {
        let relation = relation();
        let result = compare_outcome(
            &relation,
            0,
            Some(100),
            relation.confidence.as_ref(),
            &prediction(EffectDirection::Unknown),
            &outcome(EffectDirection::Increase),
        )
        .unwrap();
        assert_eq!(result.verdict, OutcomeVerdict::NoChange);
        assert!(!result.disputed);
    }
}
