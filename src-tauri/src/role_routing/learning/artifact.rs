//! Hash-checked, pure linear artifact loading. Artifacts never execute code or alter dispatch.
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const FEATURE_VERSION: &str = "rr-features-v1";

#[derive(Debug, Clone, Deserialize)]
struct LinearWeights {
    #[serde(default)]
    bias: f64,
    coefficients: BTreeMap<String, f64>,
}

#[derive(Debug, Clone)]
pub(crate) struct LinearArtifact {
    weights: LinearWeights,
}

/// Returns `None` for every untrusted artifact state. Callers retain their deterministic rules
/// result instead of treating a corrupt or mismatched artifact as a score of zero.
pub(crate) fn load_linear(
    connection: &Connection,
    artifact_id: &str,
    candidate_fingerprint: &str,
) -> Result<Option<LinearArtifact>, String> {
    let row: Option<(String, String, String, String, String, String)> = connection
        .query_row(
            "SELECT dataset_id,feature_version,candidate_fingerprint,weights_json,metrics_json,digest FROM rr_ranker_artifacts WHERE id=?1 AND algorithm='linear-v1' AND state='shadow'",
            [artifact_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some((dataset_id, feature_version, fingerprint, weights_json, metrics_json, digest)) = row
    else {
        return Ok(None);
    };
    if feature_version != FEATURE_VERSION || fingerprint != candidate_fingerprint {
        return Ok(None);
    }
    let weights: Value = serde_json::from_str(&weights_json)
        .map_err(|_| "Ranker weights are invalid".to_string())?;
    let metrics: Value = serde_json::from_str(&metrics_json)
        .map_err(|_| "Ranker metrics are invalid".to_string())?;
    let expected = sha256(format!("{dataset_id}:{fingerprint}:{}:{}", weights, metrics).as_bytes());
    if digest != expected {
        return Ok(None);
    }
    let weights: LinearWeights =
        serde_json::from_value(weights).map_err(|_| "Linear weights are invalid".to_string())?;
    if !weights.bias.is_finite()
        || weights
            .coefficients
            .values()
            .any(|value| !value.is_finite())
    {
        return Ok(None);
    }
    Ok(Some(LinearArtifact { weights }))
}

impl LinearArtifact {
    /// Scores only numeric, explicitly named features. Missing features contribute nothing.
    /// The bounded sigmoid output is a quality estimate, never an execution instruction.
    pub(crate) fn score(&self, features: &Value) -> Option<f64> {
        let object = features.as_object()?;
        let score = self.weights.coefficients.iter().try_fold(
            self.weights.bias,
            |total, (name, coefficient)| {
                let value = object.get(name).and_then(Value::as_f64).unwrap_or(0.0);
                (value.is_finite() && (total + coefficient * value).is_finite())
                    .then_some(total + coefficient * value)
            },
        )?;
        Some(1.0 / (1.0 + (-score).exp()))
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(digest: &str) -> Connection {
        let connection = Connection::open_in_memory().expect("database");
        connection.execute_batch("CREATE TABLE rr_roots(root_id TEXT PRIMARY KEY); CREATE TABLE rr_decisions(id TEXT PRIMARY KEY);").expect("base");
        super::super::schema::migrate(&connection).expect("schema");
        connection.execute("INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('d',0,'rr-features-v1','l','{}','ready',1)", []).expect("dataset");
        connection.execute("INSERT INTO rr_ranker_artifacts(id,dataset_id,algorithm,feature_version,candidate_fingerprint,weights_json,metrics_json,digest,state,created_at_ms) VALUES('a','d','linear-v1','rr-features-v1','recipes','{\"bias\":1.0,\"coefficients\":{\"complexity\":2.0}}','{}',?1,'shadow',1)", [digest]).expect("artifact");
        connection
    }

    #[test]
    fn rr_36_invalid_hash_falls_back_to_rules() {
        let connection = fixture("wrong");
        assert!(load_linear(&connection, "a", "recipes")
            .expect("load")
            .is_none());
    }

    #[test]
    fn rr_36_new_model_cold_start_and_score_are_safe() {
        let weights = serde_json::json!({"bias":1.0,"coefficients":{"complexity":2.0}});
        let digest = sha256(format!("d:recipes:{}:{}", weights, serde_json::json!({})).as_bytes());
        let connection = fixture(&digest);
        assert!(load_linear(&connection, "a", "other-recipes")
            .expect("mismatch")
            .is_none());
        let artifact = load_linear(&connection, "a", "recipes")
            .expect("load")
            .expect("artifact");
        assert_eq!(
            artifact.score(&serde_json::json!({"complexity":3.0})),
            Some(1.0 / (1.0 + (-7.0f64).exp()))
        );
    }
}
