//! Isolated evaluation of the inspector-produced TypeScript projection.
//!
//! The trusted inspector emits a self-contained TypeScript projection. This module evaluates it
//! over the enumerated boolean domain in one isolated Bun process and returns the per-case
//! outcomes for comparison with the Wasm artifact. Candidate JavaScript is never an entrypoint.

use serde_json::Value;

use super::kit::{GenerationKit, INSPECT_TIMEOUT};
use crate::generated_capabilities::inspection::comparison;
use crate::generated_capabilities::inspection::contracts::{
    InspectionError, InspectionErrorCode, InspectionResult,
};
use crate::generated_capabilities::inspection::evaluator::PrecomputedEvaluator;

const PROJECTION_DRIVER: &str = r#"import { evaluate } from "./projection.ts";
const inputs = JSON.parse(await Bun.file(process.argv[2]).text());
const results = inputs.map((input) => {
  try {
    return { value: evaluate(input) };
  } catch (error) {
    return { error: String(error) };
  }
});
console.log(JSON.stringify(results));
"#;

/// Evaluates the inspector-produced TypeScript over the enumerated boolean domain in one isolated
/// Bun process. Only the trusted projection is imported; candidate JavaScript is never an
/// entrypoint. The process is bounded by the same wall clock as inspection.
pub fn evaluate_projection(
    kit: &GenerationKit,
    typescript: &str,
    fields: &[String],
) -> InspectionResult<PrecomputedEvaluator> {
    let inputs = comparison::enumerate_inputs(fields)?;
    let directory =
        std::env::temp_dir().join(format!("saaa-projection-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&directory).map_err(|_| {
        InspectionError::new(
            InspectionErrorCode::Storage,
            "could not create the projection workspace",
        )
    })?;
    let projection_path = directory.join("projection.ts");
    let driver_path = directory.join("driver.ts");
    let inputs_path = directory.join("inputs.json");
    let prepared = (|| -> InspectionResult<()> {
        std::fs::write(&projection_path, typescript).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::Storage,
                "could not write the projection",
            )
        })?;
        std::fs::write(&driver_path, PROJECTION_DRIVER).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::Storage,
                "could not write the projection driver",
            )
        })?;
        let inputs_json = serde_json::to_vec(&inputs).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::InvalidInput,
                "the projection inputs are not serialisable",
            )
        })?;
        std::fs::write(&inputs_path, inputs_json).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::Storage,
                "could not write the projection inputs",
            )
        })
    })();
    if let Err(error) = prepared {
        let _ = std::fs::remove_dir_all(&directory);
        return Err(error);
    }
    let output = match kit.run_script(
        &driver_path,
        &[&inputs_path.to_string_lossy()],
        INSPECT_TIMEOUT,
        &directory,
    ) {
        Ok(output) if output.status.success() => output,
        _ => {
            let _ = std::fs::remove_dir_all(&directory);
            return Err(InspectionError::new(
                InspectionErrorCode::Unavailable,
                "the trusted projection process failed",
            ));
        }
    };
    let parsed: Vec<Value> = match serde_json::from_slice(&output.stdout) {
        Ok(parsed) => parsed,
        Err(_) => {
            let _ = std::fs::remove_dir_all(&directory);
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "the projection process returned invalid JSON",
            ));
        }
    };
    let _ = std::fs::remove_dir_all(&directory);
    let outcomes = parsed
        .into_iter()
        .map(|item| {
            if let Some(value) = item.get("value").and_then(Value::as_bool) {
                Ok(value)
            } else {
                Err(item
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("projection error")
                    .to_string())
            }
        })
        .collect();
    PrecomputedEvaluator::from_ordered(fields, outcomes)
}
