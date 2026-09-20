//! The management-task body for one tool invocation. Split from `invocation.rs` so the ownership
//! entry point stays small; this module owns backend execution, result storage and the terminal
//! ledger write.

#![allow(private_interfaces)]

use futures_util::FutureExt;

use super::backends::{BackendOutcome, TechnicalStatus};
use super::contracts::{now_ms, ToolSelectionError, ToolSelectionErrorCode, ToolSelectionResult};
use super::invocation::ManagedInvocation;
use super::repository;
use super::service::InvokeResponse;
use crate::persistence::SqliteWriter;

/// Runs one managed invocation to a terminal ledger state and reports the response.
pub async fn run(invocation: ManagedInvocation) -> ToolSelectionResult<InvokeResponse> {
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

/// Best-effort terminal write for a management task that panicked before settling the row.
pub fn settle_panicked(writer: &SqliteWriter, invocation_id: &str) {
    let invocation_id = invocation_id.to_string();
    let _ = writer.write(move |connection| {
        connection
            .execute(
                "UPDATE tool_selection_invocations
                    SET technical_status = 'interrupted', error_code = 'task-panic', finished_at = ?2
                  WHERE id = ?1 AND technical_status = 'running'",
                rusqlite::params![invocation_id, now_ms()],
            )
            .map_err(|_| "storage".to_string())?;
        Ok(())
    });
}
