//! Persistent assistant rendering for `/capability inspect` (plan 12.7, C12).

use serde_json::Value;

use super::CommandError;
use crate::generated_capabilities::errors::{CapabilityError, CapabilityErrorCode};
use crate::generated_capabilities::inspection::contracts::InspectionReceipt;
use crate::AppState;

#[path = "capability_inspect_store.rs"]
mod capability_inspect_store;
pub(crate) use capability_inspect_store::load_stored_inspection;

pub const MAX_DISPLAY_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct InspectionDisplay {
    pub content: String,
    #[cfg_attr(not(test), allow(dead_code))]
    pub too_large: bool,
}

pub async fn inspect_reply(
    state: &AppState,
    principal_id: &str,
    call_id: &str,
    cancellation: &crate::RunCancellation,
) -> String {
    if let Ok(receipt) = load_stored_inspection(
        &state.sqlite_writer,
        &state.data_directory,
        principal_id,
        call_id,
    ) {
        return render_reply(&receipt);
    }
    match super::capability_inspect_run::run_inspection(state, principal_id, call_id, cancellation)
        .await
    {
        Ok(receipt) => render_reply(&receipt),
        Err(error) => error_reply(error),
    }
}

fn render_reply(receipt: &InspectionReceipt) -> String {
    match render_inspection(receipt) {
        Ok(display) => format!(
            "{}\nNatural-language generation is not offered.",
            display.content.trim_end()
        ),
        Err(_) => {
            "Capability inspection could not be rendered. Natural-language generation is not offered."
                .into()
        }
    }
}

fn error_reply(error: CapabilityError) -> String {
    match error.code {
        CapabilityErrorCode::NotActive => {
            "Capability inspection is not authorized for this call. Natural-language generation is not offered."
                .into()
        }
        CapabilityErrorCode::NotValidated | CapabilityErrorCode::Unavailable => {
            "Capability inspection has not been generated for this call yet. Natural-language generation is not offered."
                .into()
        }
        CapabilityErrorCode::IntegrityError => {
            "Capability inspection evidence was altered. Natural-language generation is not offered."
                .into()
        }
        _ => {
            "Capability inspection cannot run because that call is not recorded. Natural-language generation is not offered."
                .into()
        }
    }
}

pub fn render_inspection(receipt: &InspectionReceipt) -> Result<InspectionDisplay, CommandError> {
    let summary = format!(
        "Capability inspection\nrevision: {}\nsource: {}\nprogram: {}\nartifact: {}\nprojection: {}\ncomparison: {}\n",
        receipt.revision_id,
        receipt.source_hash,
        receipt.program_hash,
        receipt.artifact_hash,
        receipt.projection_hash,
        comparison_display(&receipt.comparison_json),
    );
    let fence = fence_for(&receipt.typescript_text);
    let body = format!(
        "{summary}\n```{fence}\n{}\n```{fence}\n",
        receipt.typescript_text
    );
    if body.len() > MAX_DISPLAY_BYTES {
        return Ok(InspectionDisplay {
            content: format!(
                "{summary}\nartifact-too-large: the TypeScript projection exceeds the display limit; the managed artifact is retained.\n"
            ),
            too_large: true,
        });
    }
    Ok(InspectionDisplay {
        content: body,
        too_large: false,
    })
}

fn comparison_display(comparison: &Value) -> String {
    let checked = comparison
        .get("checkedCases")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mismatches = comparison
        .get("mismatches")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let equivalence = comparison
        .get("semanticEquivalence")
        .and_then(Value::as_str)
        .unwrap_or("not-checked");
    format!("{checked} cases checked, {mismatches} mismatches, semanticEquivalence={equivalence}")
}

pub fn fence_for(typescript: &str) -> String {
    let mut longest = 0usize;
    let mut current = 0usize;
    for character in typescript.chars() {
        if character == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

#[cfg(test)]
#[path = "capability_inspect_tests.rs"]
mod tests;
