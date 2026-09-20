//! On-demand execution of the fixed inspection for `/capability inspect` (plan 3, C05/C06).
//!
//! Ownership and existence are checked first. The trusted inspector runs once, the projection is
//! evaluated in one isolated process, the Wasm is evaluated through the trusted host, and the
//! service stores the artifacts after re-checking the revision, package integrity and contract.

use std::path::Path;
use std::sync::Arc;

use crate::generated_capabilities::errors::{CapabilityError, CapabilityErrorCode};
use crate::generated_capabilities::generation::builder::KitInspector;
use crate::generated_capabilities::generation::config;
use crate::generated_capabilities::generation::kit::GenerationKit;
use crate::generated_capabilities::generation::packager::GENERATION_WORKSPACE;
use crate::generated_capabilities::generation::projection;
use crate::generated_capabilities::generation::repository as generation_repository;
use crate::generated_capabilities::host::process::Cancellation;
use crate::generated_capabilities::inspection::comparison;
use crate::generated_capabilities::inspection::contracts::{
    InspectionContext, InspectionReceipt, InspectionReport, InspectionResult,
};
use crate::generated_capabilities::inspection::evaluator::PrecomputedEvaluator;
use crate::generated_capabilities::inspection::service::{InspectionService, Inspector};
use crate::generated_capabilities::inspection_invoke;
use crate::generated_capabilities::lifecycle;
use crate::generated_capabilities::package_store::PackageStore;
use crate::AppState;

/// Runs the fixed inspector and the projection/Wasm comparison for a recorded call, then stores
/// the artifacts. The call owner, revision, package integrity and contract hash are re-checked by
/// the service; a suspended or retired revision is still inspectable by its owner.
pub(super) async fn run_inspection(
    state: &AppState,
    principal_id: &str,
    call_id: &str,
    cancellation: &crate::RunCancellation,
) -> Result<InspectionReceipt, CapabilityError> {
    // Authorization and existence are checked before any configuration or work, so a foreign or
    // missing call is refused as not-authorized/not-recorded even when generation is disabled.
    let (package_hash, conversation_id) = resolve_call(state, principal_id, call_id)?;
    let Some(config) = config::enabled_config()? else {
        return Err(CapabilityError::new(
            CapabilityErrorCode::Unavailable,
            "generation is not configured",
        ));
    };
    let kit = Arc::new(GenerationKit::load(&config)?);
    let data_directory = &state.data_directory;
    let inspector = KitInspector::new(kit.clone(), data_directory.join(GENERATION_WORKSPACE));
    let package_directory = PackageStore::open(data_directory).package_dir(&package_hash);
    let report = inspector
        .inspect(&package_directory, &package_hash)
        .map_err(|error| error.to_capability_error())?;
    let fields: Vec<String> = report
        .contract
        .input
        .fields
        .iter()
        .map(|field| field.name.clone())
        .collect();
    let projection =
        projection::evaluate_projection(kit.as_ref(), &report.typescript.source, &fields)
            .map_err(|error| error.to_capability_error())?;
    let inputs =
        comparison::enumerate_inputs(&fields).map_err(|error| error.to_capability_error())?;
    let host_cancellation = Cancellation::default();
    let mut wasm_outcomes: Vec<Result<bool, String>> = Vec::with_capacity(inputs.len());
    for input in inputs {
        // A cancelled turn stops the comparison and frees the process slot; the generation handle
        // is then dropped without publishing partial evidence.
        if cancellation.is_cancelled() {
            host_cancellation.cancel();
            return Err(CapabilityError::new(
                CapabilityErrorCode::Cancelled,
                "inspection cancelled",
            ));
        }
        wasm_outcomes.push(
            inspection_invoke::invoke_for_inspection(
                &state.generated_capabilities,
                &package_hash,
                input,
                &host_cancellation,
            )
            .await
            .map_err(|error| error.code.as_str().to_string()),
        );
    }
    let wasm = PrecomputedEvaluator::from_ordered(&fields, wasm_outcomes)
        .map_err(|error| error.to_capability_error())?;
    let service = InspectionService::new(data_directory, kit.digest.clone());
    let context = InspectionContext {
        principal_id: principal_id.to_string(),
        conversation_id,
        project_id: None,
    };
    service
        .inspect_execution(
            &state.sqlite_writer,
            &context,
            call_id,
            &CachedInspector(report),
            &projection,
            &wasm,
        )
        .map_err(|error| error.to_capability_error())
}

/// Re-uses the report the run already produced so `inspect_execution` does not run the kit twice.
struct CachedInspector(InspectionReport);

impl Inspector for CachedInspector {
    fn inspect(
        &self,
        _package_directory: &Path,
        _package_hash: &str,
    ) -> InspectionResult<InspectionReport> {
        Ok(self.0.clone())
    }
}

/// Resolves the recorded call's revision package and conversation for the owner.
fn resolve_call(
    state: &AppState,
    principal_id: &str,
    call_id: &str,
) -> Result<(String, String), CapabilityError> {
    lifecycle::read(&state.sqlite_writer, |connection| {
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
        Ok((call.package_hash, owner.conversation_id))
    })
}
