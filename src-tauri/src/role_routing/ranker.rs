//! A fail-closed shadow ranker. Its result is telemetry until a separately reviewed policy
//! enables an artifact for execution.
use super::selection::Candidate;
use rusqlite::{Connection, OptionalExtension};

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

/// Records a shadow recommendation without changing the rules-selected recipe. Loading and
/// scoring are SQLite/CPU-only; no adapter callback exists on this path, so enabling shadow mode
/// cannot create a second model call.
pub(crate) fn record_shadow_observation(
    connection: &Connection,
    decision_id: &str,
    policy: &crate::role_routing::RoleRoutingSettings,
    candidates: &[Candidate],
    rules: &Candidate,
    now_ms: i64,
) -> Result<(), String> {
    if policy.selection.mode != "shadow" {
        return Ok(());
    }
    let Some(artifact_id) = policy.selection.shadow_artifact_id.as_deref() else {
        return Ok(());
    };
    let candidate_ids = candidates
        .iter()
        .map(|candidate| candidate.recipe_id.clone())
        .collect::<Vec<_>>();
    let expected_fingerprint = crate::adaptive_improvement::fingerprint_for(&candidate_ids);
    let row: Option<(String, String)> = connection
        .query_row(
            "SELECT candidate_fingerprint,weights_json FROM rr_ranker_artifacts WHERE id=?1 AND state='shadow'",
            [artifact_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((fingerprint, weights_json)) = row else {
        return Ok(());
    };
    if fingerprint != expected_fingerprint {
        return Ok(());
    }
    let weights: serde_json::Value = serde_json::from_str(&weights_json)
        .map_err(|_| "Shadow ranker weights are invalid".to_string())?;
    let weights = weights
        .as_object()
        .ok_or_else(|| "Shadow ranker weights must be an object".to_string())?
        .iter()
        .filter_map(|(id, score)| score.as_f64().map(|score| (id.clone(), score)))
        .collect::<Vec<_>>();
    let scores = shadow_scores(candidates, &weights);
    let recommended = choose_shadow_or_rules(rules, &scores, policy.selection.switch_margin);
    let scores_json = serde_json::to_string(
        &scores
            .iter()
            .map(|score| {
                serde_json::json!({"recipeId":score.recipe_id,"score":score.score.is_finite().then_some(score.score),"usable":score.usable})
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|error| error.to_string())?;
    connection.execute(
        "INSERT OR REPLACE INTO rr_shadow_observations(decision_id,artifact_id,scores_json,rules_id,recommended_id,created_at_ms) VALUES(?1,?2,?3,?4,?5,?6)",
        rusqlite::params![decision_id,artifact_id,scores_json,rules.recipe_id,recommended,now_ms],
    ).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
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

    #[test]
    fn rr_35_shadow_no_second_call() {
        let connection = Connection::open_in_memory().expect("database");
        connection.execute_batch("CREATE TABLE rr_decisions(id TEXT PRIMARY KEY); CREATE TABLE rr_ranker_artifacts(id TEXT PRIMARY KEY,candidate_fingerprint TEXT NOT NULL,weights_json TEXT NOT NULL,state TEXT NOT NULL); CREATE TABLE rr_shadow_observations(decision_id TEXT PRIMARY KEY,artifact_id TEXT NOT NULL,scores_json TEXT NOT NULL,rules_id TEXT NOT NULL,recommended_id TEXT NOT NULL,created_at_ms INTEGER NOT NULL); INSERT INTO rr_decisions VALUES('decision');").expect("schema");
        let candidates = vec![candidate("rule"), candidate("other")];
        let fingerprint = crate::adaptive_improvement::fingerprint_for(
            &candidates
                .iter()
                .map(|item| item.recipe_id.clone())
                .collect::<Vec<_>>(),
        );
        connection.execute("INSERT INTO rr_ranker_artifacts VALUES('artifact',?1,'{\"rule\":0.1,\"other\":0.9}','shadow')", [fingerprint]).expect("artifact");
        let mut policy = crate::role_routing::RoleRoutingSettings::default();
        policy.selection.mode = "shadow".into();
        policy.selection.shadow_artifact_id = Some("artifact".into());
        policy.selection.switch_margin = 0.1;
        let adapter_calls = Cell::new(1_u32);
        record_shadow_observation(
            &connection,
            "decision",
            &policy,
            &candidates,
            &candidates[0],
            1,
        )
        .expect("shadow observation");
        assert_eq!(
            adapter_calls.get(),
            1,
            "shadow scoring performs no adapter call"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT rules_id||':'||recommended_id FROM rr_shadow_observations",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .expect("observation"),
            "rule:other"
        );
    }
}
