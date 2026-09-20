//! v2 relevance: correlation/dependency projection, Gap candidates and Focus
//! ordering (D27/D28/D29). Pure: no DB, LLM or persistence.

use super::model_v2::{Confidence, EffectDirection, MechanismState, RelationTypeV2};
use super::slice_v2::{
    AvailabilityStateV2, ConditionStateV2, DependencyV2, GapKindV2, GapSubjectV2, ResearchGapV2,
    SliceEvidenceV2,
};
use super::traversal_v2::WorldEdgeV2;
use crate::SourceKey;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const TEMPORARY_ATTENTION: &str = "temporary_attention";

pub fn focus_rank_v2(reason: &str) -> u8 {
    match reason {
        "current_work" => 0,
        "explicit_interest" => 1,
        TEMPORARY_ATTENTION => 2,
        _ => 3,
    }
}

/// One Focus candidate for ordering. `distance` is the path length from the
/// seed; ties fall back to the stable assertion id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusCandidate {
    pub entity_id: String,
    pub reason: String,
    pub objective_assertion_id: Option<String>,
    pub distance: usize,
    pub assertion_id: String,
}

pub fn order_focus(mut candidates: Vec<FocusCandidate>) -> Vec<FocusCandidate> {
    candidates.sort_by(|a, b| {
        (
            focus_rank_v2(&a.reason),
            a.distance,
            &a.assertion_id,
            &a.entity_id,
        )
            .cmp(&(
                focus_rank_v2(&b.reason),
                b.distance,
                &b.assertion_id,
                &b.entity_id,
            ))
    });
    candidates
}

/// Correlates_with assertions that lie on a retained path. Never produces a
/// causal edge.
pub fn correlation_ids(edges: &[WorldEdgeV2], on_paths: &BTreeSet<usize>) -> Vec<String> {
    let mut ids: Vec<String> = edges
        .iter()
        .enumerate()
        .filter(|(index, edge)| {
            on_paths.contains(index) && edge.relation_type == RelationTypeV2::CorrelatesWith
        })
        .map(|(_, edge)| edge.assertion_id.clone())
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Dependencies on retained paths with the availability observed for the
/// required entity. A node merely existing in the graph is not enough.
pub fn dependencies(
    edges: &[WorldEdgeV2],
    on_paths: &BTreeSet<usize>,
    availability: &BTreeMap<String, (AvailabilityStateV2, Vec<SourceKey>)>,
) -> Vec<DependencyV2> {
    let mut out = Vec::new();
    for (index, edge) in edges.iter().enumerate() {
        if !on_paths.contains(&index) || edge.relation_type != RelationTypeV2::DependsOn {
            continue;
        }
        let (state, evidence) = availability
            .get(&edge.to)
            .cloned()
            .unwrap_or((AvailabilityStateV2::Unknown, Vec::new()));
        out.push(DependencyV2 {
            relation_id: edge.assertion_id.clone(),
            required_entity_id: edge.to.clone(),
            availability: state,
            evidence,
        });
    }
    out.sort_by(|a, b| a.relation_id.cmp(&b.relation_id));
    out.dedup_by(|a, b| a.relation_id == b.relation_id);
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GapRelationV2 {
    pub focus_entity_id: Option<String>,
    pub focus_rank: u8,
    pub path_len: usize,
    pub edge: WorldEdgeV2,
    pub evidence: Vec<SliceEvidenceV2>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeConflictV2 {
    pub relation_id: String,
    pub relation_assertion_id: String,
    pub focus_entity_id: Option<String>,
    pub focus_rank: u8,
    pub path_len: usize,
    pub evidence: Vec<SliceEvidenceV2>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownAvailabilityV2 {
    pub relation_id: String,
    pub focus_entity_id: Option<String>,
    pub focus_rank: u8,
    pub path_len: usize,
    pub evidence: Vec<SourceKey>,
}

pub struct GapInputV2<'a> {
    pub project_scope: &'a str,
    pub names: &'a BTreeMap<String, String>,
    pub relations: &'a [GapRelationV2],
    pub outcomes: &'a [OutcomeConflictV2],
    pub unknown_availability: &'a [UnknownAvailabilityV2],
    /// Normalized seed text when an explicit question could not be resolved.
    pub unresolved_seed: Option<&'a str>,
    pub explicit_question: bool,
}

fn gap_kind_code(kind: GapKindV2) -> &'static str {
    match kind {
        GapKindV2::MissingKnowledge => "missing_knowledge",
        GapKindV2::UnknownCausalDirection => "unknown_causal_direction",
        GapKindV2::MissingMechanism => "missing_mechanism",
        GapKindV2::LowConfidenceRelation => "low_confidence_relation",
        GapKindV2::MissingCondition => "missing_condition",
        GapKindV2::ConflictingEvidence => "conflicting_evidence",
    }
}

fn gap_priority(kind: GapKindV2) -> u8 {
    match kind {
        GapKindV2::ConflictingEvidence => 0,
        GapKindV2::MissingCondition => 1,
        GapKindV2::UnknownCausalDirection => 2,
        GapKindV2::MissingMechanism => 3,
        GapKindV2::LowConfidenceRelation => 4,
        GapKindV2::MissingKnowledge => 5,
    }
}

fn display_name(names: &BTreeMap<String, String>, id: &str) -> String {
    names.get(id).cloned().unwrap_or_else(|| id.to_string())
}

/// Stable key: `wmg2:` + SHA-256 of the fixed JSON array.
pub fn gap_key(
    project_scope: &str,
    kind: GapKindV2,
    entity_ids: &[String],
    relation_ids: &[String],
    normalized_seed: Option<&str>,
    conditions: &[String],
    goal_id: Option<&str>,
) -> String {
    let mut entities = entity_ids.to_vec();
    entities.sort();
    entities.dedup();
    let mut relations = relation_ids.to_vec();
    relations.sort();
    relations.dedup();
    let mut condition_keys = conditions.to_vec();
    condition_keys.sort();
    condition_keys.dedup();
    let payload = serde_json::json!([
        project_scope,
        gap_kind_code(kind),
        entities,
        relations,
        normalized_seed,
        condition_keys,
        goal_id,
    ]);
    format!(
        "wmg2:{:x}",
        Sha256::digest(serde_json::to_vec(&payload).expect("json"))
    )
}

fn question(kind: GapKindV2, name: &str, conditions: &[String]) -> String {
    let rendered = if conditions.is_empty() {
        "条件未指定".to_string()
    } else {
        conditions.join(",")
    };
    match kind {
        GapKindV2::MissingKnowledge => format!(
            "「{name}」についての情報が不足しています。許可された範囲で根拠を確認する必要があります（{rendered}）。"
        ),
        GapKindV2::UnknownCausalDirection => format!(
            "「{name}」に至る相関は存在しますが、因果方向が未確定です。介入または時系列の観測が必要です（{rendered}）。"
        ),
        GapKindV2::MissingMechanism => format!(
            "「{name}」に至る関係の機構が未記載です。媒介する仕組みの根拠が必要です（{rendered}）。"
        ),
        GapKindV2::LowConfidenceRelation => format!(
            "「{name}」に至る関係の確度が低い状態です。追加の根拠による再評価が必要です（{rendered}）。"
        ),
        GapKindV2::MissingCondition => format!(
            "「{name}」に至る関係の適用条件または依存先の可用性が未確認です（{rendered}）。"
        ),
        GapKindV2::ConflictingEvidence => format!(
            "「{name}」に至る関係で支持と反証が競合しています。どちらが現在の構成で成立するか確認が必要です（{rendered}）。"
        ),
    }
}

fn condition_labels(edge: &WorldEdgeV2) -> Vec<String> {
    let mut labels: Vec<String> = edge
        .conditions
        .iter()
        .map(|c| format!("{}={}", c.key, c.value))
        .collect();
    labels.sort();
    labels
}

/// Build Gap candidates only for relations that lie on a relevant path to a
/// Focus (or for an explicitly asked unresolved seed). Never a cross product.
pub fn build_gap_candidates(input: &GapInputV2<'_>) -> Vec<ResearchGapV2> {
    let mut candidates: Vec<(u8, u8, usize, String, ResearchGapV2)> = Vec::new();

    for relation in input.relations {
        let edge = &relation.edge;
        let focus_name = relation
            .focus_entity_id
            .as_deref()
            .map(|id| display_name(input.names, id))
            .unwrap_or_else(|| display_name(input.names, &edge.to));
        let conditions = condition_labels(edge);
        let entity_ids = vec![edge.from.clone(), edge.to.clone()];
        let relation_ids = vec![edge.assertion_id.clone()];
        let mut push = |kind: GapKindV2| {
            let key = gap_key(
                input.project_scope,
                kind,
                &entity_ids,
                &relation_ids,
                None,
                &conditions,
                None,
            );
            candidates.push((
                gap_priority(kind),
                relation.focus_rank,
                relation.path_len,
                edge.assertion_id.clone(),
                ResearchGapV2 {
                    kind,
                    subject: GapSubjectV2 {
                        entity_ids: entity_ids.clone(),
                        relation_ids: relation_ids.clone(),
                        unresolved_seed: None,
                    },
                    goal_id: None,
                    focus_entity_id: relation.focus_entity_id.clone(),
                    question: question(kind, &focus_name, &conditions),
                    key,
                    evidence: relation.evidence.clone(),
                },
            ));
        };
        if edge.epistemic == super::model_v2::Epistemic::Disputed {
            push(GapKindV2::ConflictingEvidence);
        }
        if edge.condition_state == ConditionStateV2::Unknown {
            push(GapKindV2::MissingCondition);
        }
        if edge.relation_type == RelationTypeV2::CorrelatesWith {
            push(GapKindV2::UnknownCausalDirection);
        }
        if edge.mechanism == MechanismState::Missing {
            push(GapKindV2::MissingMechanism);
        }
        if let Some(confidence) = &edge.confidence {
            if confidence.value < 500 {
                push(GapKindV2::LowConfidenceRelation);
            }
        }
    }

    for availability in input.unknown_availability {
        let focus_name = availability
            .focus_entity_id
            .as_deref()
            .map(|id| display_name(input.names, id))
            .unwrap_or_else(|| display_name(input.names, &availability.relation_id));
        let relation_ids = vec![availability.relation_id.clone()];
        let key = gap_key(
            input.project_scope,
            GapKindV2::MissingCondition,
            &[],
            &relation_ids,
            None,
            &[],
            None,
        );
        candidates.push((
            gap_priority(GapKindV2::MissingCondition),
            availability.focus_rank,
            availability.path_len,
            availability.relation_id.clone(),
            ResearchGapV2 {
                kind: GapKindV2::MissingCondition,
                subject: GapSubjectV2 {
                    entity_ids: Vec::new(),
                    relation_ids,
                    unresolved_seed: None,
                },
                goal_id: None,
                focus_entity_id: availability.focus_entity_id.clone(),
                question: question(GapKindV2::MissingCondition, &focus_name, &[]),
                key,
                evidence: availability
                    .evidence
                    .iter()
                    .map(|source| SliceEvidenceV2 {
                        source: source.clone(),
                        stance: crate::world::model::Stance::Context,
                    })
                    .collect(),
            },
        ));
    }

    for conflict in input.outcomes {
        let focus_name = conflict
            .focus_entity_id
            .as_deref()
            .map(|id| display_name(input.names, id))
            .unwrap_or_else(|| display_name(input.names, &conflict.relation_id));
        let relation_ids = vec![conflict.relation_id.clone()];
        let key = gap_key(
            input.project_scope,
            GapKindV2::ConflictingEvidence,
            &[],
            &relation_ids,
            None,
            &[],
            None,
        );
        candidates.push((
            gap_priority(GapKindV2::ConflictingEvidence),
            conflict.focus_rank,
            conflict.path_len,
            conflict.relation_id.clone(),
            ResearchGapV2 {
                kind: GapKindV2::ConflictingEvidence,
                subject: GapSubjectV2 {
                    entity_ids: Vec::new(),
                    relation_ids,
                    unresolved_seed: None,
                },
                goal_id: None,
                focus_entity_id: conflict.focus_entity_id.clone(),
                question: question(GapKindV2::ConflictingEvidence, &focus_name, &[]),
                key,
                evidence: conflict.evidence.clone(),
            },
        ));
    }

    if input.explicit_question {
        if let Some(seed) = input.unresolved_seed {
            let key = gap_key(
                input.project_scope,
                GapKindV2::MissingKnowledge,
                &[],
                &[],
                Some(seed),
                &[],
                None,
            );
            candidates.push((
                gap_priority(GapKindV2::MissingKnowledge),
                0,
                0,
                seed.to_string(),
                ResearchGapV2 {
                    kind: GapKindV2::MissingKnowledge,
                    subject: GapSubjectV2 {
                        entity_ids: Vec::new(),
                        relation_ids: Vec::new(),
                        unresolved_seed: Some(seed.to_string()),
                    },
                    goal_id: None,
                    focus_entity_id: None,
                    question: question(GapKindV2::MissingKnowledge, seed, &[]),
                    key,
                    evidence: Vec::new(),
                },
            ));
        }
    }

    candidates
        .sort_by(|a, b| (a.0, a.1, a.2, &a.3, &a.4.key).cmp(&(b.0, b.1, b.2, &b.3, &b.4.key)));
    let mut seen = BTreeSet::new();
    candidates
        .into_iter()
        .filter(|(_, _, _, _, gap)| seen.insert(gap.key.clone()))
        .map(|(_, _, _, _, gap)| gap)
        .collect()
}

/// Build a temporary-attention Focus candidate for one request only. It is
/// never persisted, and an expired current_work Objective is not demoted.
pub fn temporary_attention_focus(entity_id: &str, request_id: &str) -> FocusCandidate {
    FocusCandidate {
        entity_id: entity_id.to_string(),
        reason: TEMPORARY_ATTENTION.to_string(),
        objective_assertion_id: None,
        distance: 0,
        assertion_id: format!("request:{request_id}:{entity_id}"),
    }
}

/// Decide whether a correlation-derived effect direction is knowable. A
/// correlation alone never yields a causal direction.
pub fn correlation_direction(sign: Option<super::model_v2::CorrelationSign>) -> EffectDirection {
    match sign {
        Some(_) => EffectDirection::Unknown,
        None => EffectDirection::Unknown,
    }
}

/// A low-confidence threshold check that never treats `None` as low.
pub fn is_low_confidence(confidence: Option<&Confidence>) -> bool {
    confidence.is_some_and(|c| c.value < 500)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::model::Condition;
    use crate::world::model_v2::{ConfidenceMethod, Epistemic, RelationTypeV2};
    use crate::Status;

    fn edge(relation_type: RelationTypeV2) -> WorldEdgeV2 {
        WorldEdgeV2 {
            assertion_id: "r1".into(),
            semantic_key: "wm2:r1".into(),
            from: "a".into(),
            to: "b".into(),
            relation_type,
            lifecycle: Status::Active,
            epistemic: Epistemic::Hypothesis,
            basis: "model_hypothesis".into(),
            comparison_id: None,
            target_direction: None,
            correlation_sign: None,
            confidence: None,
            strength: None,
            evidence: Vec::new(),
            conditions: vec![Condition {
                key: "config".into(),
                value: "a".into(),
            }],
            condition_state: ConditionStateV2::Satisfied,
            mechanism: MechanismState::Unassessed,
            effect_input: None,
        }
    }

    fn names() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("a".to_string(), "A".to_string()),
            ("b".to_string(), "B".to_string()),
        ])
    }

    #[test]
    fn d28_correlation_yields_unknown_direction_not_causal_edge() {
        let mut correlation = edge(RelationTypeV2::CorrelatesWith);
        correlation.correlation_sign = Some(crate::world::model_v2::CorrelationSign::Negative);
        let input = GapInputV2 {
            project_scope: "project:p",
            names: &names(),
            relations: &[GapRelationV2 {
                focus_entity_id: Some("b".into()),
                focus_rank: 1,
                path_len: 1,
                edge: correlation,
                evidence: Vec::new(),
            }],
            outcomes: &[],
            unknown_availability: &[],
            unresolved_seed: None,
            explicit_question: false,
        };
        let gaps = build_gap_candidates(&input);
        assert!(gaps
            .iter()
            .any(|g| g.kind == GapKindV2::UnknownCausalDirection));
        // No causal edge is synthesized by the projection.
        assert_eq!(correlation_ids(&[], &BTreeSet::new()).len(), 0);
    }

    #[test]
    fn d28_null_and_500_are_not_low_confidence() {
        assert!(!is_low_confidence(None));
        assert!(!is_low_confidence(Some(&Confidence {
            value: 500,
            method: ConfidenceMethod::ManualV1,
        })));
        assert!(is_low_confidence(Some(&Confidence {
            value: 499,
            method: ConfidenceMethod::ManualV1,
        })));
    }

    #[test]
    fn d28_unresolved_seed_has_no_fabricated_relation() {
        let input = GapInputV2 {
            project_scope: "project:p",
            names: &names(),
            relations: &[],
            outcomes: &[],
            unknown_availability: &[],
            unresolved_seed: Some("unknown topic"),
            explicit_question: true,
        };
        let gaps = build_gap_candidates(&input);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].kind, GapKindV2::MissingKnowledge);
        assert!(gaps[0].subject.relation_ids.is_empty());
        assert_eq!(
            gaps[0].subject.unresolved_seed.as_deref(),
            Some("unknown topic")
        );
    }

    #[test]
    fn d29_focus_order_prefers_current_work_then_distance() {
        let ordered = order_focus(vec![
            FocusCandidate {
                entity_id: "x".into(),
                reason: "explicit_interest".into(),
                objective_assertion_id: None,
                distance: 1,
                assertion_id: "a".into(),
            },
            FocusCandidate {
                entity_id: "y".into(),
                reason: "current_work".into(),
                objective_assertion_id: Some("o1".into()),
                distance: 5,
                assertion_id: "b".into(),
            },
            temporary_attention_focus("z", "req1"),
        ]);
        assert_eq!(ordered[0].reason, "current_work");
        assert_eq!(ordered[1].reason, "explicit_interest");
        assert_eq!(ordered[2].reason, TEMPORARY_ATTENTION);
    }
}

#[cfg(test)]
mod conversion_tests {
    use super::*;
    use crate::world::model_v2::{Confidence, ConfidenceMethod, Epistemic};
    use crate::world::slice_v2::ConditionStateV2;
    use crate::Status;

    fn edge(relation_type: RelationTypeV2, confidence: Option<u16>) -> WorldEdgeV2 {
        WorldEdgeV2 {
            assertion_id: "dep1".into(),
            semantic_key: "wm2:dep1".into(),
            from: "a".into(),
            to: "b".into(),
            relation_type,
            lifecycle: Status::Active,
            epistemic: Epistemic::Hypothesis,
            basis: "model_hypothesis".into(),
            comparison_id: None,
            target_direction: None,
            correlation_sign: None,
            confidence: confidence.map(|value| Confidence {
                value,
                method: ConfidenceMethod::ManualV1,
            }),
            strength: None,
            evidence: Vec::new(),
            conditions: Vec::new(),
            condition_state: ConditionStateV2::Unknown,
            mechanism: MechanismState::Unassessed,
            effect_input: None,
        }
    }

    #[test]
    fn d27_dependency_reports_required_entity_and_availability() {
        let dependency = edge(RelationTypeV2::DependsOn, None);
        let on_paths = BTreeSet::from([0usize]);
        let availability = BTreeMap::from([(
            "b".to_string(),
            (AvailabilityStateV2::Unavailable, Vec::new()),
        )]);
        let rows = dependencies(&[dependency], &on_paths, &availability);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].required_entity_id, "b");
        assert_eq!(rows[0].availability, AvailabilityStateV2::Unavailable);
    }

    fn low_confidence_gap(confidence: Option<u16>) -> bool {
        let names = BTreeMap::from([("a".to_string(), "A".to_string())]);
        let relations = vec![GapRelationV2 {
            focus_entity_id: Some("a".into()),
            focus_rank: 1,
            path_len: 1,
            edge: edge(RelationTypeV2::Increases, confidence),
            evidence: Vec::new(),
        }];
        let input = GapInputV2 {
            project_scope: "project:p",
            names: &names,
            relations: &relations,
            outcomes: &[],
            unknown_availability: &[],
            unresolved_seed: None,
            explicit_question: false,
        };
        build_gap_candidates(&input)
            .iter()
            .any(|gap| gap.kind == GapKindV2::LowConfidenceRelation)
    }

    #[test]
    fn d28_low_confidence_boundary_is_499_not_500() {
        assert!(low_confidence_gap(Some(499)));
        assert!(!low_confidence_gap(Some(500)));
        assert!(!low_confidence_gap(None));
    }
}
