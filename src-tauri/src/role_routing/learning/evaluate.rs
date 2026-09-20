//! Offline evaluation over observed labels only. This module never invents outcomes for recipes
//! that were not actually selected in the recorded decision.
use super::artifact::LinearArtifact;
use serde_json::Value;

#[derive(Debug, Clone)]
pub(crate) struct ObservedExample {
    pub(crate) features: Value,
    pub(crate) labels: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Evaluation {
    pub(crate) observed: usize,
    pub(crate) skipped: usize,
    pub(crate) accuracy: Option<f64>,
    pub(crate) brier_score: Option<f64>,
}

/// Computes metrics only where `labels.outcome` is an explicit positive or negative observation.
/// A selected recipe id is allowed as provenance but is deliberately not extrapolated to other
/// candidates, avoiding counterfactual labels in offline replay.
pub(crate) fn evaluate_observed(
    artifact: &LinearArtifact,
    examples: &[ObservedExample],
) -> Evaluation {
    let mut correct = 0usize;
    let mut squared_error = 0.0;
    let mut observed = 0usize;
    for example in examples {
        let label = match example.labels.get("outcome").and_then(Value::as_str) {
            Some("positive") => 1.0,
            Some("negative") => 0.0,
            _ => continue,
        };
        let Some(prediction) = artifact.score(&example.features) else {
            continue;
        };
        observed += 1;
        squared_error += (prediction - label).powi(2);
        if (prediction >= 0.5) == (label >= 0.5) {
            correct += 1;
        }
    }
    Evaluation {
        observed,
        skipped: examples.len() - observed,
        accuracy: (observed != 0).then_some(correct as f64 / observed as f64),
        brier_score: (observed != 0).then_some(squared_error / observed as f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use sha2::{Digest, Sha256};

    fn artifact() -> LinearArtifact {
        let connection = Connection::open_in_memory().expect("database");
        connection
            .execute_batch("CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY); CREATE TABLE rr_decisions(id TEXT PRIMARY KEY);")
            .expect("base schema");
        super::super::schema::migrate(&connection).expect("learning schema");
        connection.execute("INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('d',0,'rr-features-v1','l','{}','ready',1)", []).expect("dataset");
        let weights = serde_json::json!({"bias":0.0,"coefficients":{"x":2.0}});
        let digest = format!(
            "{:x}",
            Sha256::digest(format!("d:recipes:{}:{}", weights, serde_json::json!({})).as_bytes())
        );
        connection.execute("INSERT INTO rr_ranker_artifacts(id,dataset_id,algorithm,feature_version,candidate_fingerprint,weights_json,metrics_json,digest,state,created_at_ms) VALUES('a','d','linear-v1','rr-features-v1','recipes',?1,'{}',?2,'shadow',1)", rusqlite::params![weights.to_string(), digest]).expect("artifact");
        super::super::artifact::load_linear(&connection, "a", "recipes")
            .expect("load")
            .expect("trusted")
    }

    #[test]
    fn rr_36_no_counterfactual_labels() {
        let evaluation = evaluate_observed(
            &artifact(),
            &[
                ObservedExample {
                    features: serde_json::json!({"x":2.0}),
                    labels: serde_json::json!({"outcome":"positive","selectedRecipeId":"observed"}),
                },
                ObservedExample {
                    features: serde_json::json!({"x":-2.0}),
                    labels: serde_json::json!({"outcome":"negative","selectedRecipeId":"observed"}),
                },
                ObservedExample {
                    features: serde_json::json!({"x":99.0}),
                    labels: serde_json::json!({"outcome":"unknown","selectedRecipeId":"never-selected"}),
                },
            ],
        );
        assert_eq!(evaluation.observed, 2);
        assert_eq!(evaluation.skipped, 1);
        assert_eq!(evaluation.accuracy, Some(1.0));
        assert!(evaluation.brier_score.is_some_and(|score| score < 0.1));
    }
}
