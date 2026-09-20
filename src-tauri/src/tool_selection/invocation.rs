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

use super::backends::{BackendRequest, ToolBackend};
use super::contracts::{RequestContext, ToolSelectionError, ToolSelectionResult};
use super::invocation_task;
use super::repository::RevisionRow;
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
    let writer = invocation.writer.clone();
    let invocation_id = invocation.invocation_id.clone();
    tokio::spawn(async move {
        // The worker catches backend panics itself, but a panic in result storage or the terminal
        // write must still not leave the ledger row `running`. This supervisor catches it, settles
        // the row, and reports an unavailable outcome to the caller.
        let response = match std::panic::AssertUnwindSafe(invocation_task::run(invocation))
            .catch_unwind()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                invocation_task::settle_panicked(&writer, &invocation_id);
                Err(ToolSelectionError::unavailable())
            }
        };
        // A detached caller is not an error; the ledger has already been settled.
        let _ = sender.send(response);
    });
    receiver
}
