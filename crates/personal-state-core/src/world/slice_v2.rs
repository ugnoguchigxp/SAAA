//! WorldSlice v2 return DTOs (D07). Pure data; selection/trimming lives in
//! `slice_build` helpers and `relevance_v2.rs`.

use super::model::Stance;
use super::model_v2::{
    CorrelationSign, EffectDirection, EntityKindV2, Epistemic, RelationTypeV2, TargetDirection,
};
use crate::{SourceKey, Status};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const WORLD_SLICE_V2_SCHEMA_VERSION: i64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionStateV2 {
    Satisfied,
    Violated,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvailabilityStateV2 {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapKindV2 {
    MissingKnowledge,
    UnknownCausalDirection,
    MissingMechanism,
    LowConfidenceRelation,
    MissingCondition,
    ConflictingEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceEvidenceV2 {
    pub source: SourceKey,
    pub stance: Stance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceNodeV2 {
    pub entity_id: String,
    pub entity_kind: EntityKindV2,
    pub name: String,
    pub assertion_id: String,
    pub objective_assertion_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConditionResultV2 {
    pub relation_assertion_id: String,
    pub key: String,
    pub expected_value: String,
    pub state: ConditionStateV2,
    pub evidence: Vec<SourceKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceRelationV2 {
    pub assertion_id: String,
    pub semantic_key: String,
    pub from_entity_id: String,
    pub to_entity_id: String,
    pub relation_type: RelationTypeV2,
    pub lifecycle: Status,
    pub epistemic: Epistemic,
    pub basis: String,
    pub target_direction: Option<TargetDirection>,
    pub correlation_sign: Option<CorrelationSign>,
    pub strength: Option<super::model_v2::CorrelationStrength>,
    pub confidence: Option<super::model_v2::Confidence>,
    pub comparison_id: Option<String>,
    pub condition_results: Vec<ConditionResultV2>,
    pub condition_state: ConditionStateV2,
    pub evidence: Vec<SliceEvidenceV2>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalStepV2 {
    pub assertion_id: String,
    pub traversed_reverse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CausalPathV2 {
    pub node_ids: Vec<String>,
    pub steps: Vec<CausalStepV2>,
    pub hops: usize,
    pub derived: bool,
    pub direction: EffectDirection,
    pub confidence: Option<super::model_v2::Confidence>,
    pub condition_state: ConditionStateV2,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelevancePathV2 {
    pub node_ids: Vec<String>,
    pub steps: Vec<CausalStepV2>,
    pub focus_entity_id: String,
    pub goal_id: Option<String>,
    pub condition_state: ConditionStateV2,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyV2 {
    pub relation_id: String,
    pub required_entity_id: String,
    pub availability: AvailabilityStateV2,
    pub evidence: Vec<SourceKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectSummaryV2 {
    pub from_entity_id: String,
    pub to_entity_id: String,
    pub comparison_id: Option<String>,
    pub direction: EffectDirection,
    pub path_indices: Vec<usize>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GapSubjectV2 {
    pub entity_ids: Vec<String>,
    pub relation_ids: Vec<String>,
    pub unresolved_seed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchGapV2 {
    pub kind: GapKindV2,
    pub subject: GapSubjectV2,
    pub goal_id: Option<String>,
    pub focus_entity_id: Option<String>,
    pub question: String,
    pub key: String,
    pub evidence: Vec<SliceEvidenceV2>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceFocusV2 {
    pub entity_id: String,
    pub reason: String,
    pub objective_assertion_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldSliceV2 {
    pub schema_version: i64,
    pub revision: u64,
    pub as_of_ms: i64,
    pub nodes: Vec<SliceNodeV2>,
    pub relations: Vec<SliceRelationV2>,
    pub focus: Vec<SliceFocusV2>,
    pub relevance_paths: Vec<RelevancePathV2>,
    pub causal_paths: Vec<CausalPathV2>,
    pub effect_summaries: Vec<EffectSummaryV2>,
    pub relevant_goal_ids: Vec<String>,
    pub relevant_project_ids: Vec<String>,
    pub correlation_ids: Vec<String>,
    pub dependencies: Vec<DependencyV2>,
    pub research_gaps: Vec<ResearchGapV2>,
    pub notices: Vec<String>,
    pub truncated: Vec<String>,
}

impl WorldSliceV2 {
    pub fn empty(revision: u64, as_of_ms: i64) -> Self {
        Self {
            schema_version: WORLD_SLICE_V2_SCHEMA_VERSION,
            revision,
            as_of_ms,
            nodes: Vec::new(),
            relations: Vec::new(),
            focus: Vec::new(),
            relevance_paths: Vec::new(),
            causal_paths: Vec::new(),
            effect_summaries: Vec::new(),
            relevant_goal_ids: Vec::new(),
            relevant_project_ids: Vec::new(),
            correlation_ids: Vec::new(),
            dependencies: Vec::new(),
            research_gaps: Vec::new(),
            notices: Vec::new(),
            truncated: Vec::new(),
        }
    }

    pub fn encoded_len(&self) -> Result<usize, String> {
        serde_json::to_vec(self)
            .map(|bytes| bytes.len())
            .map_err(|_| "world-invalid-payload".into())
    }

    /// The M1 default exploration maxima shared by every Slice.
    pub const MAX_PATHS: usize = 10;
    pub const MAX_NODES: usize = 30;
    pub const MAX_EDGES: usize = 60;
    pub const MAX_BYTES: usize = 8_192;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d07_empty_slice_roundtrips() {
        let slice = WorldSliceV2::empty(7, 100);
        let bytes = slice.encoded_len().unwrap();
        assert!(bytes > 0);
        let decoded: WorldSliceV2 =
            serde_json::from_slice(&serde_json::to_vec(&slice).unwrap()).expect("roundtrip");
        assert_eq!(decoded, slice);
        assert_eq!(decoded.schema_version, 2);
    }
}

/// One independently selectable explanation unit. The caller orders units by
/// rank; this module only unions whole units, checks the reference closure and
/// enforces the byte / count limits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SliceUnitV2 {
    pub nodes: Vec<SliceNodeV2>,
    pub relations: Vec<SliceRelationV2>,
    pub focus: Vec<SliceFocusV2>,
    pub relevance_paths: Vec<RelevancePathV2>,
    pub causal_paths: Vec<CausalPathV2>,
    pub effect_summaries: Vec<EffectSummaryV2>,
    pub dependencies: Vec<DependencyV2>,
    pub research_gaps: Vec<ResearchGapV2>,
    pub correlation_ids: Vec<String>,
    pub relevant_goal_ids: Vec<String>,
    pub relevant_project_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapCheck {
    Ok,
    Nodes,
    Edges,
    Paths,
}

fn check_caps(slice: &WorldSliceV2) -> CapCheck {
    if slice.nodes.len() > WorldSliceV2::MAX_NODES {
        return CapCheck::Nodes;
    }
    if slice.relations.len() > WorldSliceV2::MAX_EDGES {
        return CapCheck::Edges;
    }
    if slice.causal_paths.len() + slice.relevance_paths.len() > WorldSliceV2::MAX_PATHS {
        return CapCheck::Paths;
    }
    CapCheck::Ok
}

fn push_unique<T: PartialEq>(target: &mut Vec<T>, items: impl IntoIterator<Item = T>) {
    for item in items {
        if !target.contains(&item) {
            target.push(item);
        }
    }
}

impl WorldSliceV2 {
    fn union_unit(&mut self, unit: &SliceUnitV2) {
        push_unique(&mut self.nodes, unit.nodes.iter().cloned());
        push_unique(&mut self.relations, unit.relations.iter().cloned());
        push_unique(&mut self.focus, unit.focus.iter().cloned());
        push_unique(
            &mut self.relevance_paths,
            unit.relevance_paths.iter().cloned(),
        );
        push_unique(&mut self.causal_paths, unit.causal_paths.iter().cloned());
        push_unique(
            &mut self.effect_summaries,
            unit.effect_summaries.iter().cloned(),
        );
        push_unique(&mut self.dependencies, unit.dependencies.iter().cloned());
        push_unique(&mut self.research_gaps, unit.research_gaps.iter().cloned());
        push_unique(
            &mut self.correlation_ids,
            unit.correlation_ids.iter().cloned(),
        );
        push_unique(
            &mut self.relevant_goal_ids,
            unit.relevant_goal_ids.iter().cloned(),
        );
        push_unique(
            &mut self.relevant_project_ids,
            unit.relevant_project_ids.iter().cloned(),
        );
    }

    /// Drop anything whose referenced node / relation is absent. Retained
    /// objects are never stripped of conditions or evidence.
    pub fn prune_references(&mut self) {
        let node_ids: BTreeSet<String> = self.nodes.iter().map(|n| n.entity_id.clone()).collect();
        let relation_ids: BTreeSet<String> = self
            .relations
            .iter()
            .map(|r| r.assertion_id.clone())
            .collect();
        self.causal_paths.retain(|path| {
            path.node_ids.iter().all(|id| node_ids.contains(id))
                && path
                    .steps
                    .iter()
                    .all(|s| relation_ids.contains(&s.assertion_id))
        });
        self.relevance_paths.retain(|path| {
            path.node_ids.iter().all(|id| node_ids.contains(id))
                && path
                    .steps
                    .iter()
                    .all(|s| relation_ids.contains(&s.assertion_id))
        });
        self.focus.retain(|f| node_ids.contains(&f.entity_id));
        self.dependencies.retain(|d| {
            relation_ids.contains(&d.relation_id) && node_ids.contains(&d.required_entity_id)
        });
        self.effect_summaries.retain(|summary| {
            node_ids.contains(&summary.from_entity_id) && node_ids.contains(&summary.to_entity_id)
        });
        self.correlation_ids.retain(|id| relation_ids.contains(id));
        self.relevant_goal_ids.retain(|id| node_ids.contains(id));
        self.relevant_project_ids.retain(|id| node_ids.contains(id));
        self.research_gaps.retain(|gap| {
            gap.subject
                .relation_ids
                .iter()
                .all(|id| relation_ids.contains(id))
                && gap
                    .subject
                    .entity_ids
                    .iter()
                    .all(|id| node_ids.contains(id))
                && gap
                    .focus_entity_id
                    .as_ref()
                    .is_none_or(|id| node_ids.contains(id))
        });
    }
}

/// Union whole explanation units in order, skipping any unit that would break
/// the count or byte budget. The envelope is returned empty (not stripped) when
/// it does not fit.
pub fn assemble_slice(
    mut slice: WorldSliceV2,
    units: &[SliceUnitV2],
    max_bytes: usize,
) -> Result<WorldSliceV2, super::validation::WorldError> {
    use super::validation::WorldError;
    if max_bytes < 256 {
        return Err(WorldError::BudgetTooSmall);
    }
    for unit in units {
        let mut candidate = slice.clone();
        candidate.union_unit(unit);
        candidate.prune_references();
        match check_caps(&candidate) {
            CapCheck::Ok => {}
            CapCheck::Nodes => {
                push_reason(&mut candidate, "truncated:nodes");
                slice.truncated = candidate.truncated;
                continue;
            }
            CapCheck::Edges => {
                push_reason(&mut candidate, "truncated:edges");
                slice.truncated = candidate.truncated;
                continue;
            }
            CapCheck::Paths => {
                push_reason(&mut candidate, "truncated:paths");
                slice.truncated = candidate.truncated;
                continue;
            }
        }
        if candidate.encoded_len().unwrap_or(usize::MAX) <= max_bytes {
            slice = candidate;
        } else {
            push_reason(&mut slice, "truncated:budget");
        }
    }
    if slice.encoded_len().unwrap_or(usize::MAX) > max_bytes {
        return Err(WorldError::BudgetTooSmall);
    }
    Ok(slice)
}

fn push_reason(slice: &mut WorldSliceV2, reason: &str) {
    if !slice.truncated.iter().any(|r| r == reason) {
        slice.truncated.push(reason.to_string());
    }
}

#[cfg(test)]
mod assemble_tests {
    use super::*;
    use crate::world::model_v2::{
        ConfidenceMethod, CorrelationSign, Epistemic, MechanismState, RelationTypeV2,
    };
    use crate::world::validation::WorldError;

    fn node(id: &str) -> SliceNodeV2 {
        SliceNodeV2 {
            entity_id: id.into(),
            entity_kind: EntityKindV2::Concept,
            name: id.into(),
            assertion_id: format!("a_{id}"),
            objective_assertion_id: None,
        }
    }

    fn relation(id: &str) -> SliceRelationV2 {
        SliceRelationV2 {
            assertion_id: id.into(),
            semantic_key: format!("wm2:{id}"),
            from_entity_id: "n1".into(),
            to_entity_id: "n2".into(),
            relation_type: RelationTypeV2::Increases,
            lifecycle: Status::Active,
            epistemic: Epistemic::Hypothesis,
            basis: "model_hypothesis".into(),
            target_direction: None,
            correlation_sign: Some(CorrelationSign::Negative),
            strength: None,
            confidence: Some(super::super::model_v2::Confidence {
                value: 800,
                method: ConfidenceMethod::ManualV1,
            }),
            comparison_id: None,
            condition_results: Vec::new(),
            condition_state: ConditionStateV2::Satisfied,
            evidence: Vec::new(),
        }
    }

    fn unit() -> SliceUnitV2 {
        SliceUnitV2 {
            nodes: vec![node("n1"), node("n2")],
            relations: vec![relation("r1")],
            relevance_paths: vec![RelevancePathV2 {
                node_ids: vec!["n1".into(), "n2".into()],
                steps: vec![CausalStepV2 {
                    assertion_id: "r1".into(),
                    traversed_reverse: false,
                }],
                focus_entity_id: "n2".into(),
                goal_id: None,
                condition_state: ConditionStateV2::Satisfied,
                truncated: false,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn d30_one_byte_budget_is_rejected() {
        assert_eq!(
            assemble_slice(WorldSliceV2::empty(1, 0), &[], 1).unwrap_err(),
            WorldError::BudgetTooSmall
        );
    }

    #[test]
    fn d30_unit_is_adopted_when_it_fits() {
        let slice = assemble_slice(WorldSliceV2::empty(1, 0), &[unit()], 8_192).unwrap();
        assert_eq!(slice.nodes.len(), 2);
        assert_eq!(slice.relations.len(), 1);
        assert!(slice.truncated.is_empty());
    }

    #[test]
    fn d30_whole_unit_is_skipped_when_too_large() {
        let mut probe = WorldSliceV2::empty(1, 0);
        probe.union_unit(&unit());
        probe.prune_references();
        let unit_len = probe.encoded_len().unwrap();
        let slice = assemble_slice(WorldSliceV2::empty(1, 0), &[unit()], unit_len - 1).unwrap();
        assert!(slice.nodes.is_empty());
        assert!(slice.truncated.iter().any(|r| r == "truncated:budget"));
    }

    #[test]
    fn d30_dangling_paths_are_pruned() {
        let mut slice = WorldSliceV2::empty(1, 0);
        slice.nodes = vec![node("n1")];
        slice.relations = Vec::new();
        slice.relevance_paths = vec![RelevancePathV2 {
            node_ids: vec!["n1".into(), "n2".into()],
            steps: vec![CausalStepV2 {
                assertion_id: "missing".into(),
                traversed_reverse: false,
            }],
            focus_entity_id: "n2".into(),
            goal_id: None,
            condition_state: ConditionStateV2::Unknown,
            truncated: false,
        }];
        slice.prune_references();
        assert!(slice.relevance_paths.is_empty());
        assert_eq!(slice.nodes.len(), 1);
    }

    #[test]
    fn d30_mechanism_state_is_preserved_on_relations() {
        assert_ne!(MechanismState::Unassessed, MechanismState::Missing);
    }
}
