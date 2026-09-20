//! v2 semantic keys and version-crossing logical identity (D09).
//!
//! The key arrays are part of the wire contract (C2) and must not change. The
//! "logical identity" is the same array with the `wm2:`/`wm1:` prefix removed,
//! which is how a v1 and v2 assertion with the same meaning are recognized as
//! duplicates.

use super::model_v2::{
    EntityPayloadV2, FocusPayloadV2, RelationPayloadV2, RelationTypeV2,
    WORLD_V2_SEMANTIC_KEY_PREFIX,
};
use sha2::{Digest, Sha256};

fn hex_id(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hash_key(value: &serde_json::Value) -> String {
    format!(
        "{}{}",
        WORLD_V2_SEMANTIC_KEY_PREFIX,
        hex_id(&serde_json::to_vec(value).expect("json"))
    )
}

pub fn entity_key_v2(project_scope: &str, entity_id: &str) -> String {
    hash_key(&entity_identity(project_scope, entity_id))
}

pub fn entity_identity(project_scope: &str, entity_id: &str) -> serde_json::Value {
    serde_json::json!(["entity", project_scope, entity_id])
}

pub fn focus_key_v2(project_scope: &str, entity_id: &str, reason: &str) -> String {
    hash_key(&focus_identity(project_scope, entity_id, reason))
}

pub fn focus_identity(project_scope: &str, entity_id: &str, reason: &str) -> serde_json::Value {
    serde_json::json!(["focus", project_scope, entity_id, reason])
}

pub struct RelationKeyInputV2<'a> {
    pub project_scope: &'a str,
    pub from: &'a str,
    pub to: &'a str,
    pub relation_type: &'a str,
    pub effect_input: Option<&'a str>,
    pub sorted_conditions: &'a [(String, String)],
    pub comparison_id: Option<&'a str>,
    pub target_direction: Option<&'a str>,
    pub correlation_sign: Option<&'a str>,
    pub valid_from: i64,
    pub valid_until: Option<i64>,
}

fn canonical_conditions(sorted: &[(String, String)]) -> Vec<serde_json::Value> {
    sorted
        .iter()
        .map(|(k, v)| serde_json::json!([k, v]))
        .collect()
}

fn order_endpoints<'a>(relation_type: &str, from: &'a str, to: &'a str) -> (&'a str, &'a str) {
    if relation_type == "related_to" || relation_type == "correlates_with" {
        if from <= to {
            (from, to)
        } else {
            (to, from)
        }
    } else {
        (from, to)
    }
}

pub fn relation_identity(input: &RelationKeyInputV2<'_>) -> serde_json::Value {
    let (from, to) = order_endpoints(input.relation_type, input.from, input.to);
    serde_json::json!([
        "relation",
        input.project_scope,
        from,
        to,
        input.relation_type,
        input.effect_input,
        canonical_conditions(input.sorted_conditions),
        input.comparison_id,
        input.target_direction,
        input.correlation_sign,
        input.valid_from,
        input.valid_until
    ])
}

pub fn relation_key_v2(input: &RelationKeyInputV2<'_>) -> String {
    hash_key(&relation_identity(input))
}

/// Sorted `(key, value)` conditions for a v2 relation payload.
pub fn sorted_conditions(p: &RelationPayloadV2) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = p
        .conditions
        .iter()
        .map(|c| (c.key.clone(), c.value.clone()))
        .collect();
    pairs.sort();
    pairs
}

pub fn relation_identity_from_payload(
    project_scope: &str,
    p: &RelationPayloadV2,
    valid_from: i64,
    valid_until: Option<i64>,
) -> serde_json::Value {
    let conditions = sorted_conditions(p);
    relation_identity(&RelationKeyInputV2 {
        project_scope,
        from: &p.from_entity_id,
        to: &p.to_entity_id,
        relation_type: p.relation_type.as_str(),
        effect_input: p.effect_input.map(|e| e.as_str()),
        sorted_conditions: &conditions,
        comparison_id: p.comparison_id.as_deref(),
        target_direction: p.target_direction.map(|d| d.as_str()),
        correlation_sign: p.correlation_sign.map(|s| s.as_str()),
        valid_from,
        valid_until,
    })
}

pub fn relation_key_from_payload(
    project_scope: &str,
    p: &RelationPayloadV2,
    valid_from: i64,
    valid_until: Option<i64>,
) -> String {
    hash_key(&relation_identity_from_payload(
        project_scope,
        p,
        valid_from,
        valid_until,
    ))
}

pub fn entity_identity_from_payload(project_scope: &str, p: &EntityPayloadV2) -> serde_json::Value {
    entity_identity(project_scope, &p.entity_id)
}

pub fn focus_identity_from_payload(project_scope: &str, p: &FocusPayloadV2) -> serde_json::Value {
    let reason = match p.reason {
        super::model::FocusReason::CurrentWork => "current_work",
        super::model::FocusReason::ExplicitInterest => "explicit_interest",
    };
    focus_identity(project_scope, &p.entity_id, reason)
}

pub fn is_relation_correlates(relation_type: RelationTypeV2) -> bool {
    matches!(relation_type, RelationTypeV2::CorrelatesWith)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceKey;

    fn base_relation() -> RelationPayloadV2 {
        RelationPayloadV2 {
            payload_type: super::super::model::RelationTag::Relation,
            schema_version: 2,
            from_entity_id: "a".into(),
            to_entity_id: "b".into(),
            relation_type: RelationTypeV2::CorrelatesWith,
            effect_input: None,
            conditions: vec![super::super::model::Condition {
                key: "config".into(),
                value: "a".into(),
            }],
            comparison_id: None,
            basis: super::super::model::Basis::ModelHypothesis,
            evidence_stances: vec![super::super::model::EvidenceStance {
                source: SourceKey {
                    id: "s1".into(),
                    version: 1,
                    start: 0,
                    end: 1,
                },
                stance: super::super::model::Stance::Context,
            }],
            target_direction: None,
            correlation_sign: Some(super::super::model_v2::CorrelationSign::Negative),
            epistemic: super::super::model_v2::Epistemic::Hypothesis,
            confidence: None,
            strength: None,
            assessment_refs: Vec::new(),
            mechanism: super::super::model_v2::MechanismState::Unassessed,
            outcome_update: None,
        }
    }

    #[test]
    fn d09_golden_relation_key_is_fixed() {
        let conditions = vec![("config".to_string(), "a".to_string())];
        let key = relation_key_v2(&RelationKeyInputV2 {
            project_scope: "project:p",
            from: "a",
            to: "b",
            relation_type: "correlates_with",
            effect_input: None,
            sorted_conditions: &conditions,
            comparison_id: None,
            target_direction: None,
            correlation_sign: Some("negative"),
            valid_from: 100,
            valid_until: None,
        });
        assert_eq!(
            key,
            "wm2:67f6b1001cb091903064f8b29584817915b309443b6b30559f979ca2c6de9fe0"
        );
    }

    #[test]
    fn d09_correlation_endpoints_are_symmetric_but_increase_is_not() {
        let conditions = vec![("config".to_string(), "a".to_string())];
        let make = |from: &str, to: &str, relation_type: &str| {
            relation_key_v2(&RelationKeyInputV2 {
                project_scope: "project:p",
                from,
                to,
                relation_type,
                effect_input: if relation_type == "increases" {
                    Some("intervention")
                } else {
                    None
                },
                sorted_conditions: &conditions,
                comparison_id: None,
                target_direction: None,
                correlation_sign: Some("negative"),
                valid_from: 100,
                valid_until: None,
            })
        };
        assert_eq!(
            make("a", "b", "correlates_with"),
            make("b", "a", "correlates_with")
        );
        assert_ne!(make("a", "b", "increases"), make("b", "a", "increases"));
    }

    #[test]
    fn d09_confidence_change_does_not_change_identity_but_comparison_does() {
        let base = relation_identity_from_payload("project:p", &base_relation(), 100, None);
        let mut with_score = base_relation();
        with_score.confidence = Some(super::super::model_v2::Confidence {
            value: 800,
            method: super::super::model_v2::ConfidenceMethod::ManualV1,
        });
        assert_eq!(
            base,
            relation_identity_from_payload("project:p", &with_score, 100, None)
        );
        let mut with_comparison = base_relation();
        with_comparison.comparison_id = Some("cmp".into());
        assert_ne!(
            base,
            relation_identity_from_payload("project:p", &with_comparison, 100, None)
        );
    }

    #[test]
    fn d09_v1_equivalent_relation_shares_identity() {
        // v1 relation normalized to v2 has the same identity tuple.
        let v1 = super::super::model::RelationPayload {
            payload_type: super::super::model::RelationTag::Relation,
            schema_version: 1,
            from_entity_id: "a".into(),
            to_entity_id: "b".into(),
            relation_type: super::super::model::RelationType::Increases,
            effect_input: Some(super::super::model::EffectInput::Intervention),
            conditions: vec![super::super::model::Condition {
                key: "config".into(),
                value: "a".into(),
            }],
            basis: super::super::model::Basis::ModelHypothesis,
            evidence_stances: vec![super::super::model::EvidenceStance {
                source: SourceKey {
                    id: "s1".into(),
                    version: 1,
                    start: 0,
                    end: 1,
                },
                stance: super::super::model::Stance::Context,
            }],
        };
        let view = super::super::versioned::VersionedWorldPayload::V1(
            super::super::model::WorldPayload::Relation(v1),
        )
        .view("wm1:key");
        let super::super::versioned::WorldView::Relation(view) = view else {
            panic!("relation")
        };
        let from_v1 = relation_identity_from_payload("project:p", &view.payload, 100, None);
        let mut v2 = base_relation();
        v2.relation_type = RelationTypeV2::Increases;
        v2.effect_input = Some(super::super::model::EffectInput::Intervention);
        v2.correlation_sign = None;
        let from_v2 = relation_identity_from_payload("project:p", &v2, 100, None);
        assert_eq!(from_v1, from_v2);
    }
}
