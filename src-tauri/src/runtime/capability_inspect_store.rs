//! Loads a stored inspection bound to the executed call revision.

use serde_json::Value;

use crate::generated_capabilities::errors::{CapabilityError, CapabilityErrorCode};
use crate::generated_capabilities::generation::repository as generation_repository;
use crate::generated_capabilities::inspection::contracts::InspectionReceipt;
use crate::generated_capabilities::inspection::repository as inspection_repository;
use crate::generated_capabilities::inspection::service::InspectionStore;
use crate::generated_capabilities::lifecycle;

pub(crate) fn load_stored_inspection(
    writer: &crate::persistence::SqliteWriter,
    data_directory: &std::path::Path,
    principal_id: &str,
    call_id: &str,
) -> Result<InspectionReceipt, CapabilityError> {
    let row = lifecycle::read(writer, |connection| {
        let owner = generation_repository::call_owner(connection, call_id)?.ok_or_else(|| {
            CapabilityError::new(
                CapabilityErrorCode::NotActive,
                "the call has no recorded owner",
            )
        })?;
        if owner.principal_id != principal_id {
            return Err(CapabilityError::new(
                CapabilityErrorCode::NotActive,
                "the call belongs to another principal",
            ));
        }
        let call = generation_repository::call_by_id(connection, call_id)?.ok_or_else(|| {
            CapabilityError::new(
                CapabilityErrorCode::InvalidInput,
                "the call is not recorded",
            )
        })?;
        inspection_repository::latest_for_revision(connection, &call.revision_id)?.ok_or_else(
            || {
                CapabilityError::new(
                    CapabilityErrorCode::Unavailable,
                    "no inspection artifact is stored for this call",
                )
            },
        )
    })?;
    let store = InspectionStore::open(data_directory);
    let typescript_text = store
        .read_typescript(&row.relative_directory)
        .map_err(|error| error.to_capability_error())?;
    let report_bytes = store
        .read_report(&row.relative_directory)
        .map_err(|error| error.to_capability_error())?;
    let report_json: Value = serde_json::from_slice(&report_bytes).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::IntegrityError,
            "the stored inspection report is unreadable",
        )
    })?;
    let comparison_json: Value = serde_json::from_str(&row.comparison_json).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::IntegrityError,
            "the stored comparison is unreadable",
        )
    })?;
    let expected_source = report_json
        .get("typescript")
        .and_then(|value| value.get("source"))
        .and_then(Value::as_str);
    if expected_source != Some(typescript_text.as_str()) {
        return Err(CapabilityError::new(
            CapabilityErrorCode::IntegrityError,
            "the stored TypeScript projection was altered",
        ));
    }
    Ok(InspectionReceipt {
        inspection_id: row.id,
        revision_id: row.revision_id,
        source_hash: row.source_hash,
        program_hash: row.program_hash,
        artifact_hash: row.artifact_hash,
        projection_hash: row.projection_hash,
        typescript_text,
        report_json,
        comparison_json,
    })
}
