//! Ownership of one tool invocation.
//!
//! The service hands the backend call to a management task that outlives the caller future. An
//! aborted caller (a dropped provider future, an HTTP client that disconnected) must never leave
//! the ledger row `running`, drop the result on the floor, or hold a backend permit. Dropping the
//! receiver is not a cancellation: only the shared `RunCancellation` handle stops the remote call,
//! and the management task settles the invocation row exactly once either way.

#![allow(private_interfaces)]

use std::sync::Arc;

use futures_util::FutureExt;
use tokio::sync::oneshot;

use super::backends::{BackendOutcome, BackendRequest, TechnicalStatus, ToolBackend};
use super::contracts::{
    now_ms, RequestContext, ToolSelectionError, ToolSelectionErrorCode, ToolSelectionResult,
};
use super::repository::{self, RevisionRow};
use super::service::InvokeResponse;
use crate::persistence::SqliteWriter;
use crate::RunCancellation;

/// Everything the management task needs. Owned so the task is `'static` and independent of the
/// caller's stack and lifetime.
pub struct ManagedInvocation {
    pub writer: Arc<SqliteWriter>,
    pub backend: Arc<dyn ToolBackend>,
    pub invocation_id: String,
    pub context: RequestContext,
    pub tool_id: String,
    pub revision: RevisionRow,
    pub acl_epoch: i64,
    pub binding_kind: String,
    pub request: BackendRequest,
    pub cancellation: RunCancellation,
}

/// Starts the management task and returns the channel the caller awaits. Dropping the returned
/// receiver only detaches the caller; the task keeps running to termination.
pub fn spawn(
    invocation: ManagedInvocation,
) -> oneshot::Receiver<ToolSelectionResult<InvokeResponse>> {
    let (sender, receiver) = oneshot::channel();
    tokio::spawn(async move {
        let response = run(invocation).await;
        // A detached caller is not an error; the ledger has already been settled.
        let _ = sender.send(response);
    });
    receiver
}

async fn run(invocation: ManagedInvocation) -> ToolSelectionResult<InvokeResponse> {
    let ManagedInvocation {
        writer,
        backend,
        invocation_id,
        context,
        tool_id,
        revision,
        acl_epoch,
        binding_kind,
        request,
        cancellation,
    } = invocation;

    // A panic inside the backend must not skip the DB terminal write; it is reported as an
    // interrupted (indeterminate) outcome.
    let outcome = std::panic::AssertUnwindSafe(backend.invoke(request, &cancellation))
        .catch_unwind()
        .await
        .unwrap_or(BackendOutcome {
            status: TechnicalStatus::Interrupted,
            result: None,
            error_code: Some("backend-panic"),
        });

    let finalized = super::mcp::service_support::finalize_outcome(
        &writer,
        &invocation_id,
        &context,
        &tool_id,
        &revision,
        acl_epoch,
        &binding_kind,
        outcome.status,
        outcome.error_code,
        outcome.result,
    );

    let finished = now_ms();
    {
        let invocation_id = invocation_id.clone();
        let status = finalized.status;
        let error_code = finalized.error_code;
        writer
            .write(move |connection| {
                repository::finish_invocation(
                    connection,
                    &invocation_id,
                    status.as_str(),
                    error_code,
                    finished,
                )
                .map_err(|_| "storage".to_string())
            })
            .map_err(|_| ToolSelectionError::storage())?;
    }

    if finalized.status == TechnicalStatus::Cancelled {
        return Err(ToolSelectionError::new(
            ToolSelectionErrorCode::Cancelled,
            "The tool call was cancelled.",
        ));
    }
    Ok(InvokeResponse {
        invocation_id,
        status: finalized.status,
        result: finalized.result,
        error_code: finalized.error_code,
        result_ref: finalized.result_ref,
        byte_count: finalized.byte_count,
        page_count: finalized.page_count,
        result_availability: finalized.result_availability,
    })
}
