//! IO-free v2 graph traversal, signed path composition and effect summaries
//! (D24/D25/D26).

use super::model::{Condition, EffectInput};
use super::model_v2::{
    Confidence, ConfidenceMethod, CorrelationSign, CorrelationStrength, EffectDirection, Epistemic,
    MechanismState, RelationTypeV2, TargetDirection,
};
use super::slice_v2::{ConditionStateV2, SliceEvidenceV2};
use crate::Status;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_CAUSAL_DEPTH: usize = 3;
pub const MAX_RELEVANCE_DEPTH: usize = 4;
pub const MAX_NODES_V2: usize = 30;
pub const MAX_EDGES_V2: usize = 60;
pub const MAX_PATHS_V2: usize = 10;
pub const MAX_FETCH_ROWS: usize = 500;
pub const MAX_SCAN_STEPS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LimitsV2 {
    pub causal_depth: usize,
    pub relevance_depth: usize,
    pub nodes: usize,
    pub edges: usize,
    pub paths: usize,
    pub fetch_rows: usize,
    pub scan_steps: usize,
}

impl LimitsV2 {
    pub fn m1() -> Self {
        Self {
            causal_depth: MAX_CAUSAL_DEPTH,
            relevance_depth: MAX_RELEVANCE_DEPTH,
            nodes: MAX_NODES_V2,
            edges: MAX_EDGES_V2,
            paths: MAX_PATHS_V2,
            fetch_rows: MAX_FETCH_ROWS,
            scan_steps: MAX_SCAN_STEPS,
        }
    }

    /// A request may only lower each maximum.
    pub fn capped(self) -> Self {
        Self {
            causal_depth: self.causal_depth.min(MAX_CAUSAL_DEPTH),
            relevance_depth: self.relevance_depth.min(MAX_RELEVANCE_DEPTH),
            nodes: self.nodes.min(MAX_NODES_V2),
            edges: self.edges.min(MAX_EDGES_V2),
            paths: self.paths.min(MAX_PATHS_V2),
            fetch_rows: self.fetch_rows.min(MAX_FETCH_ROWS),
            scan_steps: self.scan_steps.min(MAX_SCAN_STEPS),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldEdgeV2 {
    pub assertion_id: String,
    pub semantic_key: String,
    pub from: String,
    pub to: String,
    pub relation_type: RelationTypeV2,
    pub lifecycle: Status,
    pub epistemic: Epistemic,
    pub basis: String,
    pub comparison_id: Option<String>,
    pub target_direction: Option<TargetDirection>,
    pub correlation_sign: Option<CorrelationSign>,
    pub confidence: Option<Confidence>,
    pub strength: Option<CorrelationStrength>,
    pub evidence: Vec<SliceEvidenceV2>,
    pub conditions: Vec<Condition>,
    pub condition_state: ConditionStateV2,
    pub mechanism: MechanismState,
    pub effect_input: Option<EffectInput>,
}

impl WorldEdgeV2 {
    pub fn is_causal(&self) -> bool {
        self.relation_type.is_signed_effect()
    }

    pub fn is_composable(&self) -> bool {
        self.is_causal()
            && self.lifecycle == Status::Active
            && self.epistemic != Epistemic::Disputed
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalModeV2 {
    Related,
    Causal,
    Dependencies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CausalDirection {
    Forward,
    Reverse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathStepV2 {
    pub edge: usize,
    pub traversed_reverse: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldPathV2 {
    pub nodes: Vec<String>,
    pub steps: Vec<PathStepV2>,
    pub truncated: bool,
    pub condition_state: ConditionStateV2,
}

impl WorldPathV2 {
    pub fn signature(&self) -> Vec<(usize, bool)> {
        self.steps
            .iter()
            .map(|s| (s.edge, s.traversed_reverse))
            .collect()
    }

    pub fn is_prefix_of(&self, other: &Self) -> bool {
        let a = self.signature();
        let b = other.signature();
        a.len() < b.len() && b.starts_with(&a)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TraversalV2Outcome {
    pub paths: Vec<WorldPathV2>,
    pub scanned: usize,
    pub truncated: bool,
    pub truncation_reason: Option<&'static str>,
}

struct Neighbour {
    edge: usize,
    other: String,
    traversed_reverse: bool,
}

fn eligible(edge: &WorldEdgeV2, mode: TraversalModeV2) -> bool {
    match mode {
        TraversalModeV2::Related => true,
        TraversalModeV2::Causal => edge.is_causal(),
        TraversalModeV2::Dependencies => edge.relation_type == RelationTypeV2::DependsOn,
    }
}

fn merge_state(a: ConditionStateV2, b: ConditionStateV2) -> ConditionStateV2 {
    use ConditionStateV2::*;
    match (a, b) {
        (Violated, _) | (_, Violated) => Violated,
        (Unknown, _) | (_, Unknown) => Unknown,
        _ => Satisfied,
    }
}

/// Deterministic breadth-first expansion. `Related` follows both directions;
/// `Causal` keeps the requested direction and `Dependencies` follows the
/// dependency edge from dependent to requirement.
pub fn traverse_v2(
    edges: &[WorldEdgeV2],
    seeds: &[String],
    limits: LimitsV2,
    mode: TraversalModeV2,
    direction: CausalDirection,
) -> TraversalV2Outcome {
    let depth_limit = match mode {
        TraversalModeV2::Causal => limits.causal_depth,
        _ => limits.relevance_depth,
    };
    let mut outcome = TraversalV2Outcome::default();
    let mut adjacency: BTreeMap<String, Vec<Neighbour>> = BTreeMap::new();
    for (index, edge) in edges.iter().enumerate() {
        if !eligible(edge, mode) {
            continue;
        }
        let forward = Neighbour {
            edge: index,
            other: edge.to.clone(),
            traversed_reverse: false,
        };
        let reverse = Neighbour {
            edge: index,
            other: edge.from.clone(),
            traversed_reverse: true,
        };
        match mode {
            TraversalModeV2::Related => {
                adjacency
                    .entry(edge.from.clone())
                    .or_default()
                    .push(forward);
                adjacency.entry(edge.to.clone()).or_default().push(reverse);
            }
            TraversalModeV2::Causal if direction == CausalDirection::Forward => {
                adjacency
                    .entry(edge.from.clone())
                    .or_default()
                    .push(forward);
            }
            TraversalModeV2::Causal => {
                adjacency.entry(edge.to.clone()).or_default().push(reverse);
            }
            TraversalModeV2::Dependencies => {
                adjacency
                    .entry(edge.from.clone())
                    .or_default()
                    .push(forward);
            }
        }
    }
    for list in adjacency.values_mut() {
        list.sort_by(|a, b| {
            let ea = &edges[a.edge];
            let eb = &edges[b.edge];
            (ea.relation_type, &a.other, &ea.assertion_id).cmp(&(
                eb.relation_type,
                &b.other,
                &eb.assertion_id,
            ))
        });
    }

    let mut seed_list = seeds.to_vec();
    seed_list.sort();
    seed_list.dedup();
    if seed_list.is_empty() {
        return outcome;
    }
    let mut distinct_nodes: BTreeSet<String> = seed_list.iter().cloned().collect();
    let mut collected_edges = 0usize;

    let mut queue: VecDeque<(WorldPathV2, BTreeSet<String>)> = VecDeque::new();
    for seed in &seed_list {
        queue.push_back((
            WorldPathV2 {
                nodes: vec![seed.clone()],
                steps: Vec::new(),
                truncated: false,
                condition_state: ConditionStateV2::Satisfied,
            },
            BTreeSet::from([seed.clone()]),
        ));
    }

    while let Some((path, path_visited)) = queue.pop_front() {
        let Some(current) = path.nodes.last().cloned() else {
            continue;
        };
        let Some(neighbours) = adjacency.get(&current) else {
            continue;
        };
        let depth_blocked = path.steps.len() >= depth_limit;
        for neighbour in neighbours {
            if depth_blocked {
                if !path_visited.contains(&neighbour.other) {
                    outcome.truncated = true;
                    outcome.truncation_reason.get_or_insert("depth");
                }
                continue;
            }
            if outcome.scanned >= limits.scan_steps {
                outcome.truncated = true;
                outcome.truncation_reason.get_or_insert("scan");
                break;
            }
            outcome.scanned += 1;
            if path_visited.contains(&neighbour.other) {
                continue;
            }
            if collected_edges >= limits.edges {
                outcome.truncated = true;
                outcome.truncation_reason.get_or_insert("edges");
                break;
            }
            if !distinct_nodes.contains(&neighbour.other) {
                if distinct_nodes.len() >= limits.nodes {
                    outcome.truncated = true;
                    outcome.truncation_reason.get_or_insert("nodes");
                    break;
                }
                distinct_nodes.insert(neighbour.other.clone());
            }
            let edge = &edges[neighbour.edge];
            let mut next_path = path.clone();
            next_path.nodes.push(neighbour.other.clone());
            next_path.steps.push(PathStepV2 {
                edge: neighbour.edge,
                traversed_reverse: neighbour.traversed_reverse,
            });
            next_path.condition_state =
                merge_state(next_path.condition_state, edge.condition_state);
            collected_edges += 1;
            let mut next_visited = path_visited.clone();
            next_visited.insert(neighbour.other.clone());
            outcome.paths.push(next_path.clone());
            queue.push_back((next_path, next_visited));
        }
    }
    let mut seen: BTreeSet<Vec<(usize, bool)>> = BTreeSet::new();
    outcome.paths.retain(|p| seen.insert(p.signature()));
    outcome
}

/// Drop paths that are strict prefixes of another retained path.
pub fn maximal_only_v2(paths: &[WorldPathV2]) -> Vec<WorldPathV2> {
    paths
        .iter()
        .filter(|candidate| !paths.iter().any(|other| candidate.is_prefix_of(other)))
        .cloned()
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEvaluation {
    pub direction: EffectDirection,
    pub confidence: Option<Confidence>,
    pub condition_state: ConditionStateV2,
    pub composable: bool,
}

/// Compose a signed path in its declared `from -> to` order. `hops` is the
/// number of edges.
pub fn evaluate_path(edges: &[&WorldEdgeV2]) -> PathEvaluation {
    if edges.is_empty() {
        return PathEvaluation {
            direction: EffectDirection::Unknown,
            confidence: None,
            condition_state: ConditionStateV2::Unknown,
            composable: false,
        };
    }
    let mut condition_state = ConditionStateV2::Satisfied;
    let mut all_signed = true;
    let mut all_composable = true;
    let mut comparison: Option<&str> = None;
    let mut comparison_consistent = true;
    let mut min_confidence: Option<u16> = None;
    let mut decreases = 0usize;
    for (index, edge) in edges.iter().enumerate() {
        condition_state = merge_state(condition_state, edge.condition_state);
        if !matches!(
            edge.relation_type,
            RelationTypeV2::Increases | RelationTypeV2::Decreases
        ) {
            all_signed = false;
        }
        if !edge.is_composable() {
            all_composable = false;
        }
        if edge.relation_type == RelationTypeV2::Decreases {
            decreases += 1;
        }
        match (&comparison, edge.comparison_id.as_deref()) {
            (None, Some(value)) => comparison = Some(value),
            (Some(expected), Some(value)) if *expected == value => {}
            _ => comparison_consistent = false,
        }
        if index > 0 {
            // Intermediate nodes must be metrics for a quantity chain.
            if edges[index - 1].to.as_str() != edge.from.as_str() {
                all_signed = false;
            }
        }
        match edge.confidence.as_ref().map(|c| c.value) {
            Some(value) => min_confidence = Some(min_confidence.map_or(value, |m| m.min(value))),
            None => min_confidence = None,
        }
    }
    let conditions_ok = condition_state == ConditionStateV2::Satisfied;
    let composable = all_signed
        && all_composable
        && conditions_ok
        && comparison.is_some()
        && comparison_consistent;
    if !composable {
        return PathEvaluation {
            direction: EffectDirection::Unknown,
            confidence: None,
            condition_state,
            composable: false,
        };
    }
    let direction = if decreases % 2 == 1 {
        EffectDirection::Decrease
    } else {
        EffectDirection::Increase
    };
    let confidence = min_confidence.map(|min| Confidence {
        value: path_rank(min as u64, edges.len()),
        method: ConfidenceMethod::PathRankV1,
    });
    PathEvaluation {
        direction,
        confidence,
        condition_state,
        composable: true,
    }
}

/// `floor(min * 9^(h-1) / 10^(h-1))` in integer arithmetic, one final division.
pub fn path_rank(min: u64, hops: usize) -> u16 {
    if hops == 0 {
        return min.min(1000) as u16;
    }
    let exponent = (hops - 1) as u32;
    let numerator = min.saturating_mul(9u64.saturating_pow(exponent));
    let denominator = 10u64.saturating_pow(exponent);
    (numerator / denominator).min(1000) as u16
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectSummary {
    pub from_entity_id: String,
    pub to_entity_id: String,
    pub comparison_id: Option<String>,
    pub direction: EffectDirection,
    pub path_indices: Vec<usize>,
    pub complete: bool,
}

/// Aggregate signed paths with the same endpoints and comparison basis. A
/// contradiction is reported only in the summary; individual directions stay.
pub fn effect_summary(
    paths: &[WorldPathV2],
    edges: &[WorldEdgeV2],
    truncated: bool,
) -> Vec<EffectSummary> {
    type GroupKey = (String, String, Option<String>);
    type GroupEntry = (usize, EffectDirection);
    let mut groups: BTreeMap<GroupKey, Vec<GroupEntry>> = BTreeMap::new();
    for (index, path) in paths.iter().enumerate() {
        let Some(first) = path.steps.first() else {
            continue;
        };
        let Some(last) = path.steps.last() else {
            continue;
        };
        let (Some(from), Some(to)) = (path.nodes.first(), path.nodes.last()) else {
            continue;
        };
        let edge_refs: Vec<&WorldEdgeV2> = path.steps.iter().map(|s| &edges[s.edge]).collect();
        let evaluation = evaluate_path(&edge_refs);
        let comparison = edges[first.edge]
            .comparison_id
            .clone()
            .or_else(|| edges[last.edge].comparison_id.clone());
        groups
            .entry((from.clone(), to.clone(), comparison))
            .or_default()
            .push((index, evaluation.direction));
    }
    groups
        .into_iter()
        .map(|((from, to, comparison), entries)| {
            let mut directions: BTreeSet<&'static str> = BTreeSet::new();
            for (_, direction) in &entries {
                directions.insert(direction.as_str());
            }
            let direction = if directions.len() > 1 {
                EffectDirection::Mixed
            } else {
                entries
                    .first()
                    .map(|(_, d)| *d)
                    .unwrap_or(EffectDirection::Unknown)
            };
            EffectSummary {
                from_entity_id: from,
                to_entity_id: to,
                comparison_id: comparison,
                direction,
                path_indices: entries.into_iter().map(|(i, _)| i).collect(),
                complete: !truncated,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::model::Condition;

    fn edge(
        id: &str,
        from: &str,
        to: &str,
        relation_type: RelationTypeV2,
        confidence: Option<u16>,
        comparison: Option<&str>,
    ) -> WorldEdgeV2 {
        WorldEdgeV2 {
            assertion_id: id.into(),
            semantic_key: format!("wm2:{id}"),
            from: from.into(),
            to: to.into(),
            relation_type,
            lifecycle: Status::Active,
            epistemic: Epistemic::Hypothesis,
            basis: "model_hypothesis".into(),
            comparison_id: comparison.map(|c| c.into()),
            target_direction: None,
            correlation_sign: None,
            confidence: confidence.map(|value| Confidence {
                value,
                method: ConfidenceMethod::ManualV1,
            }),
            strength: None,
            evidence: Vec::new(),
            conditions: vec![Condition {
                key: "config".into(),
                value: "a".into(),
            }],
            condition_state: ConditionStateV2::Satisfied,
            mechanism: MechanismState::Unassessed,
            effect_input: Some(EffectInput::Intervention),
        }
    }

    #[test]
    fn d25_three_hop_scores_567_and_decreases() {
        let a = edge(
            "e1",
            "a",
            "b",
            RelationTypeV2::Increases,
            Some(900),
            Some("c"),
        );
        let b = edge(
            "e2",
            "b",
            "c",
            RelationTypeV2::Decreases,
            Some(800),
            Some("c"),
        );
        let c = edge(
            "e3",
            "c",
            "d",
            RelationTypeV2::Increases,
            Some(700),
            Some("c"),
        );
        let evaluation = evaluate_path(&[&a, &b, &c]);
        assert_eq!(evaluation.direction, EffectDirection::Decrease);
        assert_eq!(evaluation.confidence.unwrap().value, 567);
        assert!(evaluation.composable);
    }

    #[test]
    fn d25_different_comparison_is_unknown_and_null() {
        let a = edge(
            "e1",
            "a",
            "b",
            RelationTypeV2::Increases,
            Some(900),
            Some("c1"),
        );
        let b = edge(
            "e2",
            "b",
            "c",
            RelationTypeV2::Decreases,
            Some(800),
            Some("c2"),
        );
        let evaluation = evaluate_path(&[&a, &b]);
        assert_eq!(evaluation.direction, EffectDirection::Unknown);
        assert!(evaluation.confidence.is_none());
    }

    #[test]
    fn d25_unverified_conditions_stay_null() {
        let mut a = edge(
            "e1",
            "a",
            "b",
            RelationTypeV2::Increases,
            Some(900),
            Some("c"),
        );
        a.condition_state = ConditionStateV2::Unknown;
        let b = edge(
            "e2",
            "b",
            "c",
            RelationTypeV2::Increases,
            Some(800),
            Some("c"),
        );
        let evaluation = evaluate_path(&[&a, &b]);
        assert!(evaluation.confidence.is_none());
        assert!(!evaluation.composable);
    }

    #[test]
    fn d24_causal_reverse_does_not_follow_forward() {
        let e = edge("e1", "a", "b", RelationTypeV2::Increases, None, None);
        let forward = traverse_v2(
            std::slice::from_ref(&e),
            &["a".into()],
            LimitsV2::m1(),
            TraversalModeV2::Causal,
            CausalDirection::Forward,
        );
        assert_eq!(forward.paths.len(), 1);
        let reverse = traverse_v2(
            &[e],
            &["a".into()],
            LimitsV2::m1(),
            TraversalModeV2::Causal,
            CausalDirection::Reverse,
        );
        assert!(reverse.paths.is_empty());
    }

    #[test]
    fn d26_goal_reachable_through_metrics_and_project() {
        let e1 = edge("e1", "tech", "m1", RelationTypeV2::Increases, None, None);
        let e2 = edge("e2", "m1", "m2", RelationTypeV2::Increases, None, None);
        let e3 = {
            let mut e = edge("e3", "m2", "g1", RelationTypeV2::ServesGoal, None, None);
            e.effect_input = None;
            e
        };
        let e4 = {
            let mut e = edge("e4", "p1", "g1", RelationTypeV2::HasGoal, None, None);
            e.effect_input = None;
            e
        };
        let outcome = traverse_v2(
            &[e1, e2, e3, e4],
            &["tech".into()],
            LimitsV2::m1(),
            TraversalModeV2::Related,
            CausalDirection::Forward,
        );
        let four_hop = outcome
            .paths
            .iter()
            .find(|p| p.steps.len() == 4)
            .expect("4 hop path");
        assert_eq!(four_hop.nodes.last().unwrap(), "p1");
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;
    use crate::world::model::Condition;

    fn edge(id: &str, from: &str, to: &str, direction: RelationTypeV2) -> WorldEdgeV2 {
        WorldEdgeV2 {
            assertion_id: id.into(),
            semantic_key: format!("wm2:{id}"),
            from: from.into(),
            to: to.into(),
            relation_type: direction,
            lifecycle: crate::Status::Active,
            epistemic: Epistemic::Hypothesis,
            basis: "model_hypothesis".into(),
            comparison_id: Some("cmp".into()),
            target_direction: None,
            correlation_sign: None,
            confidence: Some(Confidence {
                value: 800,
                method: ConfidenceMethod::ManualV1,
            }),
            strength: None,
            evidence: Vec::new(),
            conditions: vec![Condition {
                key: "config".into(),
                value: "a".into(),
            }],
            condition_state: ConditionStateV2::Satisfied,
            mechanism: MechanismState::Unassessed,
            effect_input: Some(EffectInput::Intervention),
        }
    }

    fn path(edges: &[(usize, bool)], nodes: &[&str]) -> WorldPathV2 {
        WorldPathV2 {
            nodes: nodes.iter().map(|n| n.to_string()).collect(),
            steps: edges
                .iter()
                .map(|(edge, reverse)| PathStepV2 {
                    edge: *edge,
                    traversed_reverse: *reverse,
                })
                .collect(),
            truncated: false,
            condition_state: ConditionStateV2::Satisfied,
        }
    }

    #[test]
    fn d25_opposing_directions_are_mixed_only_in_the_summary() {
        let edges = vec![
            edge("up", "a", "b", RelationTypeV2::Increases),
            edge("down", "a", "b", RelationTypeV2::Decreases),
        ];
        let paths = vec![
            path(&[(0, false)], &["a", "b"]),
            path(&[(1, false)], &["a", "b"]),
        ];
        let summaries = effect_summary(&paths, &edges, false);
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].direction, EffectDirection::Mixed);
        assert_eq!(summaries[0].path_indices.len(), 2);
        // The individual path directions are preserved.
        assert_eq!(
            evaluate_path(&[&edges[0]]).direction,
            EffectDirection::Increase
        );
        assert_eq!(
            evaluate_path(&[&edges[1]]).direction,
            EffectDirection::Decrease
        );
    }
}
