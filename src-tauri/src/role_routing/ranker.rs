//! A fail-closed shadow ranker. Its result is telemetry until a separately reviewed policy
//! enables an artifact for execution.
use super::selection::Candidate;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ShadowScore {
    pub(crate) recipe_id: String,
    pub(crate) score: f64,
    pub(crate) usable: bool,
}

pub(crate) fn shadow_scores(
    candidates: &[Candidate],
    weights: &[(String, f64)],
) -> Vec<ShadowScore> {
    candidates
        .iter()
        .map(|candidate| {
            let score = weights
                .iter()
                .find(|(id, _)| id == &candidate.recipe_id)
                .map(|(_, score)| *score)
                .unwrap_or(f64::NAN);
            ShadowScore {
                recipe_id: candidate.recipe_id.clone(),
                score,
                usable: candidate.exclusion_reason.is_none() && score.is_finite(),
            }
        })
        .collect()
}

/// Invalid or tied shadow outputs retain the rules choice. This function never returns an
/// excluded candidate and it has no side effects.
pub(crate) fn choose_shadow_or_rules(
    rules: &Candidate,
    scores: &[ShadowScore],
    margin: f64,
) -> String {
    let mut usable = scores.iter().filter(|s| s.usable).collect::<Vec<_>>();
    usable.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.recipe_id.cmp(&b.recipe_id))
    });
    let Some(best) = usable.first() else {
        return rules.recipe_id.clone();
    };
    let rule_score = scores
        .iter()
        .find(|s| s.recipe_id == rules.recipe_id)
        .filter(|s| s.usable)
        .map(|s| s.score)
        .unwrap_or(best.score);
    if best.recipe_id != rules.recipe_id && best.score - rule_score >= margin {
        best.recipe_id.clone()
    } else {
        rules.recipe_id.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(id: &str) -> Candidate {
        Candidate {
            recipe_id: id.into(),
            actor_ids: vec!["a".into()],
            exclusion_reason: None,
            reason_codes: vec![],
        }
    }
    #[test]
    fn rr_35_invalid_ranker_falls_back_to_rules() {
        let rule = candidate("rule");
        let scores = shadow_scores(&[rule.clone()], &[("rule".into(), f64::NAN)]);
        assert_eq!(choose_shadow_or_rules(&rule, &scores, 0.1), "rule");
    }
    #[test]
    fn rr_06_sticky_actor_keeps_rules_without_margin() {
        let rule = candidate("rule");
        let scores = shadow_scores(
            &[rule.clone(), candidate("other")],
            &[("rule".into(), 0.5), ("other".into(), 0.55)],
        );
        assert_eq!(choose_shadow_or_rules(&rule, &scores, 0.1), "rule");
    }
}
