//! Precomputed case evaluators for the projection/Wasm comparison.
//!
//! The projection runs in one isolated process and the Wasm runs through the trusted host; their
//! per-case outcomes are collected in `enumerate_inputs` order and replayed here so the pure
//! comparison harness stays free of processes and async runtimes.

use serde_json::{Map, Value};
use std::collections::HashMap;

use super::comparison::{enumerate_inputs, CaseEvaluator};
use super::contracts::{InspectionError, InspectionErrorCode, InspectionResult};

/// An evaluator whose per-case outcomes were precomputed (for example by one isolated process run
/// for the projection and one host run per case for the Wasm). The lookup is keyed by the canonical
/// encoding of the input produced by [`enumerate_inputs`].
pub struct PrecomputedEvaluator {
    results: HashMap<String, Result<bool, String>>,
}

impl PrecomputedEvaluator {
    /// Builds an evaluator from outcomes aligned with `enumerate_inputs(fields)`.
    pub fn from_ordered(
        fields: &[String],
        outcomes: Vec<Result<bool, String>>,
    ) -> InspectionResult<Self> {
        let inputs = enumerate_inputs(fields)?;
        if inputs.len() != outcomes.len() {
            return Err(InspectionError::new(
                InspectionErrorCode::InvalidInput,
                "comparison result count does not match the input domain",
            ));
        }
        let results = inputs
            .into_iter()
            .zip(outcomes)
            .map(|(input, outcome)| (input_key(&input), outcome))
            .collect();
        Ok(Self { results })
    }
}

impl CaseEvaluator for PrecomputedEvaluator {
    fn evaluate(&self, input: &Map<String, Value>) -> Result<bool, String> {
        self.results
            .get(&input_key(input))
            .cloned()
            .unwrap_or_else(|| Err("missing precomputed case".into()))
    }
}

fn input_key(input: &Map<String, Value>) -> String {
    serde_json::to_string(input).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated_capabilities::inspection::comparison::compare;

    struct Product;

    impl CaseEvaluator for Product {
        fn evaluate(&self, input: &Map<String, Value>) -> Result<bool, String> {
            Ok(input.values().all(|value| value.as_bool() == Some(true)))
        }
    }

    #[test]
    fn precomputed_results_replay_in_enumerated_order() {
        let fields = vec!["enabled".to_string(), "suspended".to_string()];
        // enumerate order: 00, 10, 01, 11 -> AND is false,false,false,true
        let evaluator = PrecomputedEvaluator::from_ordered(
            &fields,
            vec![Ok(false), Ok(false), Ok(false), Ok(true)],
        )
        .unwrap();
        let result = compare(
            &fields,
            &Product,
            &evaluator,
            &"a".repeat(64),
            &"b".repeat(64),
        )
        .unwrap();
        assert!(result.is_match());
    }

    #[test]
    fn mismatched_result_count_is_refused() {
        let fields = vec!["enabled".to_string()];
        assert!(PrecomputedEvaluator::from_ordered(&fields, vec![Ok(true)]).is_err());
    }

    #[test]
    fn errors_are_preserved_as_mismatches() {
        let fields = vec!["enabled".to_string()];
        let evaluator = PrecomputedEvaluator::from_ordered(
            &fields,
            vec![Err("boom".into()), Err("boom".into())],
        )
        .unwrap();
        let result = compare(
            &fields,
            &evaluator,
            &evaluator,
            &"a".repeat(64),
            &"b".repeat(64),
        )
        .unwrap();
        // Both sides error identically, which the harness counts as a mismatch, never a pass.
        assert!(!result.is_match());
    }
}
