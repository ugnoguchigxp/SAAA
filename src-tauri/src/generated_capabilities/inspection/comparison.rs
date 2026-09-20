//! Projection/Wasm comparison harness (plan 3, C06).
//!
//! The trusted L-Lang projection is evaluated over the full boolean input domain (at most 256
//! cases) and compared with the Wasm result for the same inputs. This is a finite comparison,
//! never a proof of semantic equivalence: the result keeps `semanticEquivalence: not-checked`.

use serde::Serialize;
use serde_json::{Map, Value};

use super::super::limits::MAX_CONTRACT_FIELDS;
use super::contracts::{InspectionError, InspectionErrorCode, InspectionResult};

pub const MAX_COMPARISON_CASES: usize = 256;
pub const COMPARISON_DOMAIN: &str = "all-boolean-inputs";

/// One evaluator (trusted projection process or Wasm host) over the enumerated domain.
pub trait CaseEvaluator {
    fn evaluate(&self, input: &Map<String, Value>) -> Result<bool, String>;
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonMismatch {
    pub case_id: String,
    pub input: Map<String, Value>,
    pub projection: Option<bool>,
    pub wasm: Option<bool>,
    pub projection_error: bool,
    pub wasm_error: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonResult {
    pub checked_cases: usize,
    pub mismatches: Vec<ComparisonMismatch>,
    pub input_domain: &'static str,
    pub projection_hash: String,
    pub artifact_hash: String,
    pub semantic_equivalence: &'static str,
    pub display: String,
}

impl ComparisonResult {
    pub fn is_match(&self) -> bool {
        self.mismatches.is_empty()
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Enumerates the 2^n boolean input domain in the contract's fixed field order.
pub fn enumerate_inputs(fields: &[String]) -> InspectionResult<Vec<Map<String, Value>>> {
    if fields.is_empty() || fields.len() > MAX_CONTRACT_FIELDS {
        return Err(InspectionError::new(
            InspectionErrorCode::InvalidInput,
            "comparison input field count is out of range",
        ));
    }
    let total = 1usize << fields.len();
    if total > MAX_COMPARISON_CASES {
        return Err(InspectionError::new(
            InspectionErrorCode::InvalidInput,
            "comparison domain exceeds 256 cases",
        ));
    }
    let mut cases = Vec::with_capacity(total);
    for index in 0..total {
        let mut input = Map::new();
        for (bit, field) in fields.iter().enumerate() {
            input.insert(field.clone(), Value::Bool((index >> bit) & 1 == 1));
        }
        cases.push(input);
    }
    Ok(cases)
}

/// Compares the two evaluators over the full domain. Any evaluator error is a mismatch, never a
/// pass; a single mismatch fails the whole comparison.
pub fn compare(
    fields: &[String],
    projection: &dyn CaseEvaluator,
    wasm: &dyn CaseEvaluator,
    projection_hash: &str,
    artifact_hash: &str,
) -> InspectionResult<ComparisonResult> {
    let cases = enumerate_inputs(fields)?;
    let mut mismatches = Vec::new();
    for (index, input) in cases.iter().enumerate() {
        let projection_outcome = projection.evaluate(input);
        let wasm_outcome = wasm.evaluate(input);
        let projection_value = projection_outcome.as_ref().ok().copied();
        let wasm_value = wasm_outcome.as_ref().ok().copied();
        if projection_outcome.is_err() || wasm_outcome.is_err() || projection_value != wasm_value {
            mismatches.push(ComparisonMismatch {
                case_id: format!("case-{index}"),
                input: input.clone(),
                projection: projection_value,
                wasm: wasm_value,
                projection_error: projection_outcome.is_err(),
                wasm_error: wasm_outcome.is_err(),
            });
        }
    }
    let display = if mismatches.is_empty() {
        format!(
            "The projection and Wasm agree on the specified boolean input domain ({} cases); semantic equivalence is not proven.",
            cases.len()
        )
    } else {
        format!(
            "{} of {} cases disagree between the projection and Wasm; the inspection fails.",
            mismatches.len(),
            cases.len()
        )
    };
    Ok(ComparisonResult {
        checked_cases: cases.len(),
        mismatches,
        input_domain: COMPARISON_DOMAIN,
        projection_hash: projection_hash.to_string(),
        artifact_hash: artifact_hash.to_string(),
        semantic_equivalence: "not-checked",
        display,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Constant(bool);

    impl CaseEvaluator for Constant {
        fn evaluate(&self, _input: &Map<String, Value>) -> Result<bool, String> {
            Ok(self.0)
        }
    }

    struct Product;

    impl CaseEvaluator for Product {
        fn evaluate(&self, input: &Map<String, Value>) -> Result<bool, String> {
            let value = input.values().all(|value| value.as_bool() == Some(true));
            Ok(value)
        }
    }

    struct FailingAt(String);

    impl CaseEvaluator for FailingAt {
        fn evaluate(&self, input: &Map<String, Value>) -> Result<bool, String> {
            if input.get(&self.0).and_then(Value::as_bool) == Some(true) {
                Err("boom".into())
            } else {
                Ok(false)
            }
        }
    }

    #[test]
    fn the_domain_is_every_boolean_combination_in_field_order() {
        let fields = vec!["enabled".to_string(), "suspended".to_string()];
        let cases = enumerate_inputs(&fields).unwrap();
        assert_eq!(cases.len(), 4);
        let rendered = cases
            .iter()
            .map(|input| {
                format!(
                    "{}{}",
                    if input["enabled"].as_bool().unwrap() {
                        "1"
                    } else {
                        "0"
                    },
                    if input["suspended"].as_bool().unwrap() {
                        "1"
                    } else {
                        "0"
                    }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(rendered, vec!["00", "10", "01", "11"]);
    }

    #[test]
    fn eight_fields_produce_exactly_256_cases() {
        let fields = (0..8).map(|index| format!("f{index}")).collect::<Vec<_>>();
        assert_eq!(enumerate_inputs(&fields).unwrap().len(), 256);
    }

    #[test]
    fn mismatches_are_never_reported_as_a_pass() {
        let fields = vec!["enabled".to_string()];
        let result = compare(
            &fields,
            &Constant(true),
            &Constant(false),
            &"a".repeat(64),
            &"b".repeat(64),
        )
        .unwrap();
        assert!(!result.is_match());
        assert_eq!(result.mismatches.len(), 2);
        assert_eq!(result.semantic_equivalence, "not-checked");
    }

    #[test]
    fn matching_evaluators_report_the_domain_and_keep_the_disclaimer() {
        let fields = vec!["enabled".to_string(), "suspended".to_string()];
        let result = compare(
            &fields,
            &Product,
            &Product,
            &"a".repeat(64),
            &"b".repeat(64),
        )
        .unwrap();
        assert!(result.is_match());
        assert_eq!(result.checked_cases, 4);
        assert_eq!(result.input_domain, "all-boolean-inputs");
        assert!(result.display.contains("not proven"));
    }

    #[test]
    fn evaluator_errors_are_mismatches() {
        let fields = vec!["enabled".to_string()];
        let result = compare(
            &fields,
            &FailingAt("enabled".into()),
            &Constant(false),
            &"a".repeat(64),
            &"b".repeat(64),
        )
        .unwrap();
        assert!(!result.is_match());
        assert!(result.mismatches[0].projection_error);
    }

    #[test]
    fn over_eight_fields_is_refused() {
        let fields = (0..9).map(|index| format!("f{index}")).collect::<Vec<_>>();
        assert!(enumerate_inputs(&fields).is_err());
    }
}
