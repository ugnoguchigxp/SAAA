//! Test-only hooks on [`CapabilityService`]. They live in a child module so the production file
//! stays focused; a child module can reach the service's private fields and methods.

use tokio::sync::OwnedSemaphorePermit;

use super::{CapabilityService, ImportCandidate, RevisionRef};
use crate::generated_capabilities::{
    errors::CapabilityResult,
    execution,
    host::process::{self, Cancellation},
};

impl CapabilityService {
    /// Occupies the single process slot so capacity limits can be observed.
    pub fn occupy_process_slot(&self) -> CapabilityResult<OwnedSemaphorePermit> {
        self.acquire_process_slot()
    }

    /// Holds admission open so a caller can be parked before it registers its execution.
    pub async fn acquire_admission(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.admission.lock().await
    }

    /// Registry entries currently owned by the service.
    pub fn active_executions(&self) -> usize {
        execution::active_count(&self.executions)
    }

    /// Registers a cancellation as an owned execution.
    pub fn register_owned_execution(&self, id: &str, cancellation: &Cancellation) {
        self.register_execution(id, cancellation);
    }

    /// Releases an execution registered by `register_owned_execution`.
    pub fn unregister_owned_execution(&self, id: &str) {
        execution::unregister(&self.executions, id);
    }

    /// Runs the import against a scripted host command, so cancellation of a running import can
    /// be observed deterministically.
    pub async fn import_candidate_with_command(
        &self,
        candidate: &ImportCandidate,
        command: process::RuntimeCommand,
    ) -> CapabilityResult<RevisionRef> {
        self.import_candidate_with(candidate, Some(command)).await
    }
}
