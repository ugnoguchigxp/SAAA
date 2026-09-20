//! Pure retrieval/ranking math: cosine, RRF, base-score normalization, conditional soft
//! correction and a stable topological sort for explicit pairwise rules. No database and no
//! inference calls live here so the numeric golden tests are deterministic.

use super::contracts::*;
use std::collections::{BTreeSet, HashMap};

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.is_empty() || a.len() != b.len() {
        return None;
    }
    let mut dot = 0.0_f64;
    let mut norm_a = 0.0_f64;
    let mut norm_b = 0.0_f64;
    for (left, right) in a.iter().zip(b.iter()) {
        let left = f64::from(*left);
        let right = f64::from(*right);
        if !left.is_finite() || !right.is_finite() {
            return None;
        }
        dot += left * right;
        norm_a += left * left;
        norm_b += right * right;
    }
    if norm_a <= 0.0 || norm_b <= 0.0 {
        return None;
    }
    let value = dot / (norm_a.sqrt() * norm_b.sqrt());
    value.is_finite().then_some(value.clamp(-1.0, 1.0))
}

/// Reciprocal-rank fusion. Ranks are 1-based; a missing branch contributes zero. Ties are not
/// broken here — callers sort by (score desc, tool id asc, revision id asc).
pub fn rrf_score(lex_rank: Option<usize>, vec_rank: Option<usize>) -> f64 {
    let lexical = lex_rank
        .map(|rank| 1.0 / (RRF_K + rank as f64))
        .unwrap_or(0.0);
    let vector = vec_rank
        .map(|rank| 1.0 / (RRF_K + rank as f64))
        .unwrap_or(0.0);
    lexical + vector
}

#[derive(Clone, Debug, PartialEq)]
pub struct FusedCandidate {
    pub revision_id: String,
    pub lex_rank: Option<usize>,
    pub vec_rank: Option<usize>,
    pub score: f64,
}

/// Fuses the lexical and vector top lists. Both inputs are ordered best-first and rank 1 means
/// the top item. Deterministic ties use revision id ascending.
pub fn fuse(lexical: &[String], vector: &[String], top: usize) -> Vec<FusedCandidate> {
    let lex_rank: HashMap<&str, usize> = lexical
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index + 1))
        .collect();
    let vec_rank: HashMap<&str, usize> = vector
        .iter()
        .enumerate()
        .map(|(index, id)| (id.as_str(), index + 1))
        .collect();
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    ids.extend(lexical.iter().map(String::as_str));
    ids.extend(vector.iter().map(String::as_str));
    let mut fused: Vec<FusedCandidate> = ids
        .into_iter()
        .map(|id| {
            let lex = lex_rank.get(id).copied();
            let vec = vec_rank.get(id).copied();
            FusedCandidate {
                revision_id: id.to_string(),
                lex_rank: lex,
                vec_rank: vec,
                score: rrf_score(lex, vec),
            }
        })
        .collect();
    fused.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.revision_id.cmp(&right.revision_id))
    });
    fused.truncate(top);
    fused
}

/// Cross-encoder rank to a bounded base score in `[0, 1]`. `rank` is 1-based.
pub fn base_from_rank(rank: usize, total: usize) -> f64 {
    if total <= 1 {
        return 1.0;
    }
    let rank = rank.max(1);
    1.0 - (rank as f64 - 1.0) / (total as f64 - 1.0)
}

/// Applies `-0.25`/`+0.25` per rule and clamps the total correction for one target to `[-0.5, 0.5]`.
pub fn weighted_correction(actions: &[RuleAction]) -> f64 {
    let mut total = 0.0;
    for action in actions {
        match action {
            RuleAction::Avoid => total -= RULE_STRENGTH,
            RuleAction::Prefer => total += RULE_STRENGTH,
            RuleAction::Pairwise | RuleAction::Forbid => {}
        }
    }
    total.clamp(-RULE_CORRECTION_CLAMP, RULE_CORRECTION_CLAMP)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairwiseOutcome {
    Sorted,
    Cycle,
}

/// Stable topological sort driven by pairwise preferences. `edges` are `(preferred, rejected)`
/// tool IDs. Among unconstrained nodes the original soft order is preserved. Cycles are reported
/// so the caller can decide whether to supersede an older rule or return ambiguity.
pub fn apply_pairwise(
    ordered: &[CorrectedCandidate],
    edges: &[(String, String)],
) -> (Vec<CorrectedCandidate>, PairwiseOutcome) {
    let index: HashMap<&str, usize> = ordered
        .iter()
        .enumerate()
        .map(|(position, candidate)| (candidate.tool_id.as_str(), position))
        .collect();
    let relevant: Vec<(usize, usize)> = edges
        .iter()
        .filter_map(|(preferred, rejected)| {
            let from = *index.get(preferred.as_str())?;
            let to = *index.get(rejected.as_str())?;
            (from != to).then_some((from, to))
        })
        .collect();
    if relevant.is_empty() {
        return (ordered.to_vec(), PairwiseOutcome::Sorted);
    }

    let mut adjacency: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); ordered.len()];
    let mut indegree = vec![0_usize; ordered.len()];
    for (from, to) in &relevant {
        if adjacency[*from].insert(*to) {
            indegree[*to] += 1;
        }
    }
    // Priority queue keyed by the original soft position keeps unconstrained candidates stable.
    let mut ready: Vec<usize> = (0..ordered.len())
        .filter(|node| indegree[*node] == 0)
        .collect();
    ready.sort_unstable();
    let mut output = Vec::with_capacity(ordered.len());
    let mut consumed = vec![false; ordered.len()];
    while let Some(position) = ready.first().copied() {
        ready.remove(0);
        if consumed[position] {
            continue;
        }
        consumed[position] = true;
        output.push(ordered[position].clone());
        for next in adjacency[position].iter().copied() {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                let insert_at = ready.partition_point(|node| *node < next);
                ready.insert(insert_at, next);
            }
        }
    }
    if output.len() != ordered.len() {
        return (ordered.to_vec(), PairwiseOutcome::Cycle);
    }
    (output, PairwiseOutcome::Sorted)
}

/// Final ordering: score descending, then tool id ascending, then revision id ascending.
pub fn sort_corrected(candidates: &mut [CorrectedCandidate]) {
    candidates.sort_by(|left, right| {
        right
            .final_score
            .partial_cmp(&left.final_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.tool_id.cmp(&right.tool_id))
            .then_with(|| left.revision_id.cmp(&right.revision_id))
    });
}

/// Clamps a free-form score for storage. `NaN`/`Inf` is reported as `None` so the caller can
/// fail the inference batch instead of persisting a corrupt score.
pub fn finite_score(value: f64) -> Option<f64> {
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(tool_id: &str, base: f64) -> CorrectedCandidate {
        CorrectedCandidate {
            revision_id: format!("{tool_id}-rev"),
            tool_id: tool_id.to_string(),
            base_score: base,
            correction: 0.0,
            final_score: base,
            rule_ids: Vec::new(),
        }
    }

    #[test]
    fn cosine_is_bounded_and_rejects_shape_mismatch() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]), Some(1.0));
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), Some(0.0));
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), None);
        assert_eq!(cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), None);
    }

    #[test]
    fn cosine_ignores_magnitude() {
        let left = cosine_similarity(&[2.0, 0.0], &[5.0, 0.0]).expect("finite");
        assert!((left - 1.0).abs() < 1e-9);
    }

    #[test]
    fn rrf_uses_one_based_ranks_and_missing_branches_are_zero() {
        let only_lex = rrf_score(Some(1), None);
        let only_vec = rrf_score(None, Some(1));
        assert!((only_lex - only_vec).abs() < 1e-12);
        assert!((only_lex - 1.0 / 61.0).abs() < 1e-12);
    }

    #[test]
    fn fusion_breaks_ties_by_revision_id() {
        let fused = fuse(&["b".into(), "a".into()], &[], 10);
        assert_eq!(fused[0].revision_id, "b");
        assert_eq!(fused[1].revision_id, "a");
    }

    #[test]
    fn base_is_one_and_symmetric_for_three_candidates() {
        assert_eq!(base_from_rank(1, 3), 1.0);
        assert_eq!(base_from_rank(2, 3), 0.5);
        assert_eq!(base_from_rank(3, 3), 0.0);
        assert_eq!(base_from_rank(1, 1), 1.0);
    }

    #[test]
    fn golden_avoid_and_prefer_produce_a_tie_broken_by_id() {
        // A base 1.0, B base 0.5, C base 0.0. A is avoided, B is preferred.
        let mut candidates = vec![
            candidate("A", 1.0),
            candidate("B", 0.5),
            candidate("C", 0.0),
        ];
        candidates[0].correction = weighted_correction(&[RuleAction::Avoid]);
        candidates[0].final_score = candidates[0].base_score + candidates[0].correction;
        candidates[1].correction = weighted_correction(&[RuleAction::Prefer]);
        candidates[1].final_score = candidates[1].base_score + candidates[1].correction;
        candidates[2].final_score = 0.0;
        sort_corrected(&mut candidates);
        let order: Vec<&str> = candidates
            .iter()
            .map(|item| item.tool_id.as_str())
            .collect();
        assert_eq!(order, vec!["A", "B", "C"]); // both 0.75, tie by ID
    }

    #[test]
    fn golden_pairwise_inserts_b_before_a() {
        let candidates = vec![
            candidate("A", 1.0),
            candidate("B", 0.5),
            candidate("C", 0.0),
        ];
        let (sorted, outcome) = apply_pairwise(&candidates, &[("B".to_string(), "A".to_string())]);
        assert_eq!(outcome, PairwiseOutcome::Sorted);
        let order: Vec<&str> = sorted.iter().map(|item| item.tool_id.as_str()).collect();
        assert_eq!(order, vec!["B", "A", "C"]);
    }

    #[test]
    fn golden_mismatched_condition_keeps_base_order() {
        let mut candidates = vec![
            candidate("A", 1.0),
            candidate("B", 0.5),
            candidate("C", 0.0),
        ];
        // No matching rule means no correction.
        sort_corrected(&mut candidates);
        let order: Vec<&str> = candidates
            .iter()
            .map(|item| item.tool_id.as_str())
            .collect();
        assert_eq!(order, vec!["A", "B", "C"]);
    }

    #[test]
    fn clamped_correction_never_exceeds_half() {
        let correction = weighted_correction(&[
            RuleAction::Prefer,
            RuleAction::Prefer,
            RuleAction::Prefer,
            RuleAction::Prefer,
        ]);
        assert!((correction - 0.5).abs() < 1e-12);
        let correction =
            weighted_correction(&[RuleAction::Avoid, RuleAction::Avoid, RuleAction::Avoid]);
        assert!((correction + 0.5).abs() < 1e-12);
    }

    #[test]
    fn pairwise_cycle_is_reported_not_silently_reordered() {
        let candidates = vec![candidate("A", 1.0), candidate("B", 0.9)];
        let (_, outcome) = apply_pairwise(
            &candidates,
            &[
                ("A".to_string(), "B".to_string()),
                ("B".to_string(), "A".to_string()),
            ],
        );
        assert_eq!(outcome, PairwiseOutcome::Cycle);
    }
}
