//! Inspection-only package invocation (plan 3, 12.4).
//!
//! The inspection service has already re-checked the call owner, the fixed revision and the
//! managed package integrity. This runs the exact managed artifact directly, without consulting
//! the active pointer, so a suspended or retired revision can still be inspected by its owner.

use serde_json::{Map, Value};

use super::contracts::{HostOutcome, HostRequest, OperationResult};
use super::errors::{error, CapabilityErrorCode, CapabilityResult};
use super::host::process::Cancellation;
use super::{limits, service::CapabilityService};

pub(crate) async fn invoke_for_inspection(
    service: &CapabilityService,
    package_hash: &str,
    input: Map<String, Value>,
    cancellation: &Cancellation,
) -> CapabilityResult<bool> {
    let host = service.require_host()?.clone();
    service.ensure_accepting()?;
    let _permit = service.acquire_process_slot()?;
    let manifest_path = service.store().manifest_path(package_hash);
    let request = HostRequest::invoke(
        "inspection-compare",
        package_hash,
        input,
        limits::INNER_TIMEOUT_MAX_MS,
    );
    let response = host.execute(&request, &manifest_path, cancellation).await?;
    match response.outcome {
        HostOutcome::Ok(OperationResult::Invoke(value)) => Ok(value),
        HostOutcome::Error(code) => error(code.capability_code(), "inspection invoke failed"),
        _ => error(
            CapabilityErrorCode::ProtocolError,
            "unexpected inspection result",
        ),
    }
}
