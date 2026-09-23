//! Ownership of one accepted execution: the cancellation registry, the abandonment guard, and
//! the detached host task.
//!
//! A caller that goes away (task abort, disconnect) must not leave a `running` record behind,
//! and the host process must still be reclaimed. The detached task owns the process permit and
//! the terminal write, so dropping the caller's future neither loses the record nor releases the
//! slot before the record is stored.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex as SyncMutex},
    time::Instant,
};
use tokio::sync::OwnedSemaphorePermit;

use super::{
    contracts::{HostOutcome, HostRequest, OperationResult},
    errors::*,
    host::{process::Cancellation, WasmHost},
    repository,
    service::InvocationResult,
};
use crate::{now_iso, persistence::SqliteWriter};

pub(crate) type ExecutionRegistry = Arc<SyncMutex<HashMap<String, Cancellation>>>;

pub(crate) fn new_registry() -> ExecutionRegistry {
    Arc::new(SyncMutex::new(HashMap::new()))
}

pub(crate) fn register(registry: &ExecutionRegistry, id: &str, cancellation: &Cancellation) {
    let mut guard = registry.lock().unwrap_or_else(|error| error.into_inner());
    guard.insert(id.to_string(), cancellation.clone());
}

pub(crate) fn unregister(registry: &ExecutionRegistry, id: &str) {
    let mut guard = registry.lock().unwrap_or_else(|error| error.into_inner());
    guard.remove(id);
}

pub(crate) fn outstanding(registry: &ExecutionRegistry) -> Vec<Cancellation> {
    let guard = registry.lock().unwrap_or_else(|error| error.into_inner());
    guard.values().cloned().collect()
}

pub(crate) fn active_count(registry: &ExecutionRegistry) -> usize {
    let guard = registry.lock().unwrap_or_else(|error| error.into_inner());
    guard.len()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecutionKind {
    Check,
    Import,
}

/// Moves a record left `running`/`staging` by an abandoned future to a terminal status. Only
/// rows that are still in the non-terminal status are touched, so a completed record is never
/// overwritten.
pub(crate) fn interrupt(writer: &SqliteWriter, kind: ExecutionKind, id: &str) {
    let now = now_iso();
    let _ = writer.write(|connection| {
        match kind {
            ExecutionKind::Check => repository::interrupt_check(connection, id, &now),
            ExecutionKind::Import => repository::interrupt_import(connection, id, &now),
        }
        .map_err(|error| error.encode())?;
        Ok(())
    });
}

/// Owns the registry entry of one accepted execution. Dropping it releases the entry and moves a
/// record that never reached a terminal status to `interrupted`.
pub(crate) struct ExecutionGuard<'a> {
    pub(super) registry: &'a ExecutionRegistry,
    pub(super) writer: &'a SqliteWriter,
    pub(super) id: String,
    pub(super) kind: ExecutionKind,
    pub(super) settled: bool,
}

impl<'a> ExecutionGuard<'a> {
    pub(crate) fn new(
        registry: &'a ExecutionRegistry,
        writer: &'a SqliteWriter,
        id: &str,
        kind: ExecutionKind,
    ) -> Self {
        Self {
            registry,
            writer,
            id: id.to_string(),
            kind,
            settled: false,
        }
    }

    /// Marks the execution as having reached its terminal state on the normal path.
    pub fn settle(&mut self) {
        self.settled = true;
    }
}

impl Drop for ExecutionGuard<'_> {
    fn drop(&mut self) {
        unregister(self.registry, &self.id);
        if !self.settled {
            interrupt(self.writer, self.kind, &self.id);
        }
    }
}

/// One detached host invocation. It owns the process permit, so an aborted caller cannot release
/// the execution slot before the terminal record is stored.
pub(crate) struct HostInvocation {
    pub(crate) host: Arc<WasmHost>,
    pub(crate) writer: Arc<SqliteWriter>,
    pub(crate) registry: ExecutionRegistry,
    pub(crate) cancellation: Cancellation,
    pub(crate) call_id: String,
    pub(crate) revision_id: String,
    pub(crate) package_hash: String,
    pub(crate) request: HostRequest,
    pub(crate) manifest: PathBuf,
    pub(crate) started: Instant,
    /// Held so the execution slot stays busy until the terminal record is stored.
    pub(crate) _permit: Option<OwnedSemaphorePermit>,
}

impl HostInvocation {
    pub(crate) async fn run(self) -> CapabilityResult<InvocationResult> {
        let response = self
            .host
            .execute(&self.request, &self.manifest, &self.cancellation)
            .await;
        let elapsed_ms = self.started.elapsed().as_millis() as u64;
        let (status, stored_value, error_code, outcome) = match response {
            Ok(response) => match response.outcome {
                HostOutcome::Ok(OperationResult::Invoke(value)) => {
                    ("succeeded", Some(value), None, Ok(value))
                }
                HostOutcome::Error(host_error) => {
                    let code = host_error.capability_code();
                    ("failed", None, Some(code), Err(code))
                }
                _ => {
                    let code = CapabilityErrorCode::ProtocolError;
                    ("failed", None, Some(code), Err(code))
                }
            },
            Err(failure) => {
                let status = if failure.code == CapabilityErrorCode::Cancelled {
                    "cancelled"
                } else {
                    "failed"
                };
                (status, None, Some(failure.code), Err(failure.code))
            }
        };
        let stored = record(
            &self.writer,
            &self.call_id,
            status,
            stored_value,
            error_code,
        );
        unregister(&self.registry, &self.call_id);
        // `self` (and with it the process permit) is dropped when this function returns.
        match (stored, outcome) {
            // A record that could not be stored is never reported as success; startup recovery
            // moves the still-running row to `interrupted`.
            (Err(error), _) => Err(error),
            (Ok(()), Ok(value)) => Ok(InvocationResult {
                call_id: self.call_id.clone(),
                revision_id: self.revision_id.clone(),
                package_hash: self.package_hash.clone(),
                value,
                elapsed_ms,
            }),
            (Ok(()), Err(code)) => Err(CapabilityError::new(code, "host invocation failed")),
        }
    }
}

fn record(
    writer: &SqliteWriter,
    call_id: &str,
    status: &str,
    result: Option<bool>,
    error_code: Option<CapabilityErrorCode>,
) -> CapabilityResult<()> {
    let now = now_iso();
    writer
        .write(|connection| {
            repository::finish_call(
                connection,
                call_id,
                status,
                result,
                error_code.map(|code| code.as_str()),
                &now,
            )
            .map_err(|error| error.encode())
        })
        .map_err(CapabilityError::decode)
}
