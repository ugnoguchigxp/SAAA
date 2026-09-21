//! The management-task body for one tool invocation. Split from `invocation.rs` so the ownership
//! entry point stays small; this module owns backend execution, result storage and the terminal
//! ledger write.

#![allow(private_interfaces)]

use futures_util::FutureExt;
use rusqlite::OptionalExtension;

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
        let revision_id = revision.id.clone();
        let status = finalized.status;
        let error_code = finalized.error_code;
        writer
            .write(move |connection| {
                let transaction = connection
                    .unchecked_transaction()
                    .map_err(|_| "storage".to_string())?;
                repository::finish_invocation(
                    &transaction,
                    &invocation_id,
                    status.as_str(),
                    error_code,
                    finished,
                )
                .map_err(|_| "storage".to_string())?;
                // Only the policy-selected candidate receives this label. A user can invoke a
                // lower ranked candidate from the same search result, which is not evidence that
                // the ranker's top choice succeeded or failed.
                let adaptive_schema_present: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='ai_decisions')",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|_| "storage".to_string())?;
                if adaptive_schema_present {
                    let decision: Option<(String, String)> = transaction
                        .query_row(
                            "SELECT d.id,d.selected FROM tool_selection_invocations i JOIN ai_decisions d ON d.id=('ai-tool-' || i.decision_id) WHERE i.id=?1",
                            [&invocation_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .optional()
                        .map_err(|_| "storage".to_string())?;
                    if let Some((decision_id, selected_revision_id)) = decision {
                        if selected_revision_id == revision_id {
                            crate::adaptive_improvement::record_outcome_in_transaction(
                                &transaction,
                                &decision_id,
                                Some(status == TechnicalStatus::Succeeded),
                                None,
                                None,
                                None,
                                None,
                                None,
                                None,
                                &invocation_id,
                                0,
                                finished,
                            )?;
                        }
                    }
                }
                transaction.commit().map_err(|_| "storage".to_string())
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
