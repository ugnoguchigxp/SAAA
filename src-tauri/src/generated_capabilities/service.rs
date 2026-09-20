use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{oneshot, Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

use super::{
    contracts::{self, HostOutcome, HostRequest, OperationResult, PackageManifest, WasmContract},
    errors::*,
    execution::{self, ExecutionGuard, ExecutionKind, ExecutionRegistry, HostInvocation},
    guards::{activate_revision, finalize_check, validate_input, CheckOutcome},
    host::{
        process::{self, Cancellation},
        runtime_bundle::{self, RuntimeConfig},
        WasmHost,
    },
    lifecycle, limits,
    package_store::{PackageStore, StagedPackage},
    repository::{self, RevisionState},
    verification::{self, AcceptanceLedger},
};
use crate::{now_iso, persistence::SqliteWriter};

/// How long `shutdown` waits for owned host processes to be reaped.
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(test)]
mod test_support;

#[derive(Clone, Debug)]
pub struct ImportCandidate {
    pub capability_id: Option<String>,
    pub candidate_directory: PathBuf,
    pub acceptance_id: String,
    pub provenance: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevisionRef {
    pub capability_id: String,
    pub revision_id: String,
    pub package_hash: String,
}

#[derive(Clone, Debug)]
pub struct VerificationSummary {
    pub check_id: String,
    pub passed: bool,
    pub error_code: Option<CapabilityErrorCode>,
    pub detail: String,
    pub report_ref: Option<String>,
}

pub use super::contracts::{InvocationResult, InvokeRequest, ResolvedCapability};

/// The single service behind M1 management and execution. Later milestones plug conversation
/// tools and MCP into it; they do not get their own catalog or current pointer.
pub struct CapabilityService {
    writer: Arc<SqliteWriter>,
    store: PackageStore,
    ledger: AcceptanceLedger,
    host: Option<Arc<WasmHost>>,
    exposure_enabled: bool,
    unavailable: Option<CapabilityError>,
    admission: AsyncMutex<()>,
    process: Arc<Semaphore>,
    shutting_down: AtomicBool,
    executions: ExecutionRegistry,
}

impl CapabilityService {
    /// A missing or invalid runtime configuration disables the feature without preventing SAAA
    /// from starting.
    pub(crate) fn build(
        writer: Arc<SqliteWriter>,
        data_directory: &Path,
        ledger_directory: PathBuf,
        config: Option<RuntimeConfig>,
    ) -> Self {
        let store = PackageStore::open(data_directory);
        let (host, exposure_enabled, unavailable) = match config {
            None => (
                None,
                false,
                Some(CapabilityError::new(
                    CapabilityErrorCode::Disabled,
                    "runtime configuration is not configured",
                )),
            ),
            Some(config) => match runtime_bundle::validate(&config) {
                Ok(runtime) => match WasmHost::new(runtime, config.bun_path.clone()) {
                    Ok(host) => (Some(Arc::new(host)), config.enabled, None),
                    Err(error) => (None, false, Some(error)),
                },
                Err(error) => (None, false, Some(error)),
            },
        };
        Self {
            writer,
            store,
            ledger: AcceptanceLedger::new(ledger_directory),
            host,
            exposure_enabled,
            unavailable,
            admission: AsyncMutex::new(()),
            process: Arc::new(Semaphore::new(1)),
            shutting_down: AtomicBool::new(false),
            executions: execution::new_registry(),
        }
    }

    pub(crate) fn writer(&self) -> &Arc<SqliteWriter> {
        &self.writer
    }

    pub fn store(&self) -> &PackageStore {
        &self.store
    }

    pub fn is_ready(&self) -> bool {
        self.host.is_some()
    }

    pub fn is_exposed(&self) -> bool {
        self.exposure_enabled && self.host.is_some()
    }

    /// Stops accepting new host work, cancels every owned execution, and waits (bounded) until
    /// the single process slot is released. Returns whether the service drained cleanly: the slot
    /// is free, no execution is still registered, and no record was left non-terminal.
    pub fn shutdown(&self) -> bool {
        self.shutting_down.store(true, Ordering::SeqCst);
        for cancellation in execution::outstanding(&self.executions) {
            cancellation.cancel();
        }
        let deadline = Instant::now() + SHUTDOWN_DRAIN_TIMEOUT;
        while self.process.available_permits() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.process.available_permits() == 1
            && execution::active_count(&self.executions) == 0
            && self.unsettled_rows() == 0
    }

    fn unsettled_rows(&self) -> i64 {
        lifecycle::read(&self.writer, repository::unsettled_rows).unwrap_or(i64::MAX)
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }

    pub fn unavailable_reason(&self) -> Option<&CapabilityError> {
        self.unavailable.as_ref()
    }

    fn ensure_accepting(&self) -> CapabilityResult<()> {
        if self.is_shutting_down() {
            return error(
                CapabilityErrorCode::Unavailable,
                "the capability service is shutting down",
            );
        }
        Ok(())
    }

    fn register_execution(&self, id: &str, cancellation: &Cancellation) {
        execution::register(&self.executions, id, cancellation);
    }

    fn require_host(&self) -> CapabilityResult<&Arc<WasmHost>> {
        match (&self.host, &self.unavailable) {
            (Some(host), _) => Ok(host),
            (None, Some(error)) => Err(error.clone()),
            (None, None) => error(
                CapabilityErrorCode::Disabled,
                "generated capabilities are disabled",
            ),
        }
    }

    fn require_exposure(&self) -> CapabilityResult<()> {
        if self.exposure_enabled {
            Ok(())
        } else {
            error(
                CapabilityErrorCode::Disabled,
                "generated capability exposure is disabled",
            )
        }
    }

    fn acquire_process_slot(&self) -> CapabilityResult<OwnedSemaphorePermit> {
        self.process.clone().try_acquire_owned().map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::Busy,
                "another host process is already running",
            )
        })
    }

    /// Staging, size/path checks, trusted inspect, publish and catalog insert.
    pub async fn import_candidate(
        &self,
        candidate: &ImportCandidate,
    ) -> CapabilityResult<RevisionRef> {
        self.import_candidate_with(candidate, None).await
    }

    async fn import_candidate_with(
        &self,
        candidate: &ImportCandidate,
        command: Option<process::RuntimeCommand>,
    ) -> CapabilityResult<RevisionRef> {
        let host = self.require_host()?.clone();
        self.ensure_accepting()?;
        // Import spawns a host process, so it shares the single execution slot and the
        // cancellation registry with verify and invoke.
        let _permit = self.acquire_process_slot()?;
        let acceptance = self.ledger.resolve(&candidate.acceptance_id)?;
        let import_id = crate::new_id("gcimport");
        // The registered token is the one the host inspect runs with, so a shutdown (or an
        // abandoned caller) actually cancels a running import.
        let cancellation = Cancellation::default();
        self.register_execution(&import_id, &cancellation);
        let mut guard = ExecutionGuard::new(
            &self.executions,
            &self.writer,
            &import_id,
            ExecutionKind::Import,
        );
        let result = self
            .import_candidate_inner(
                host,
                candidate,
                acceptance,
                import_id.clone(),
                &cancellation,
                command,
            )
            .await;
        if result.is_ok() {
            guard.settle();
        }
        result
    }

    async fn import_candidate_inner(
        &self,
        host: Arc<WasmHost>,
        candidate: &ImportCandidate,
        acceptance: verification::AcceptanceRef,
        import_id: String,
        cancellation: &Cancellation,
        command: Option<process::RuntimeCommand>,
    ) -> CapabilityResult<RevisionRef> {
        let staging = self.store.create_staging(&import_id)?;
        self.record_import_staging(&import_id)?;

        let staged = match self.store.stage(&candidate.candidate_directory, &staging) {
            Ok(staged) => staged,
            Err(error) => return self.refuse_import(&import_id, &staging, error),
        };
        let inspection = match self
            .inspect_candidate(
                &host,
                &staged,
                &staging.join("capability.json"),
                cancellation,
                command,
            )
            .await
        {
            Ok(inspection) => inspection,
            Err(error) => return self.refuse_import(&import_id, &staging, error),
        };
        if let Err(error) = self.validate_inspection(&staged, &inspection) {
            return self.refuse_import(&import_id, &staging, error);
        }
        // The acceptance ledger names the capability it grades. Binding it to the candidate's
        // metadata id stops a revision from being verified against another capability that
        // merely happens to share the same input contract.
        if acceptance.capability_id != staged.manifest.metadata.id {
            return self.refuse_import(
                &import_id,
                &staging,
                CapabilityError::new(
                    CapabilityErrorCode::IntegrityError,
                    "acceptance reference belongs to another capability",
                ),
            );
        }
        let inventory_hash = PackageStore::inventory_hash(&staged.inventory);
        if let Err(error) = self.store.finalize(&staged) {
            self.record_import_failure(&import_id, &error);
            return Err(error);
        }

        let revision = lifecycle::transaction(&self.writer, |transaction| {
            let now = now_iso();
            let capability = repository::ensure_capability(
                transaction,
                candidate.capability_id.as_deref(),
                &staged.manifest.metadata.id,
                &now,
            )?;
            if let Some(existing) = repository::revision_by_package_hash(
                transaction,
                &capability.id,
                &staged.package_hash,
            )? {
                if existing.required_acceptance_hash != acceptance.hash {
                    return error(
                        CapabilityErrorCode::Conflict,
                        "the same package hash is already registered with different acceptance",
                    );
                }
                repository::finish_import(
                    transaction,
                    &import_id,
                    "completed",
                    Some(&existing.id),
                    None,
                    &now,
                )?;
                return Ok(RevisionRef {
                    capability_id: capability.id,
                    revision_id: existing.id,
                    package_hash: existing.package_hash,
                });
            }
            let revision_id = uuid::Uuid::new_v4().to_string();
            repository::insert_revision(
                transaction,
                &repository::RevisionInsert {
                    id: revision_id.clone(),
                    capability_id: capability.id.clone(),
                    package_hash: staged.package_hash.clone(),
                    inventory_hash: inventory_hash.clone(),
                    contract_hash: contracts::contract_hash(&inspection.contract),
                    runtime_digest: host.runtime_digest().to_string(),
                    required_acceptance_hash: acceptance.hash.clone(),
                    provenance_json: serde_json::json!({ "source": candidate.provenance })
                        .to_string(),
                    manifest_json: serde_json::to_string(&staged.manifest_value)
                        .map_err(|_| storage_error("manifest is not serialisable"))?,
                    contract_json: serde_json::to_string(&inspection.contract)
                        .map_err(|_| storage_error("contract is not serialisable"))?,
                    metadata_json: serde_json::to_string(&staged.manifest.metadata)
                        .map_err(|_| storage_error("metadata is not serialisable"))?,
                    created_at: now.clone(),
                },
            )?;
            repository::finish_import(
                transaction,
                &import_id,
                "completed",
                Some(&revision_id),
                None,
                &now,
            )?;
            Ok(RevisionRef {
                capability_id: capability.id,
                revision_id,
                package_hash: staged.package_hash.clone(),
            })
        });

        match revision {
            Ok(revision) => Ok(revision),
            Err(error) => {
                self.record_import_failure(&import_id, &error);
                Err(error)
            }
        }
    }

    async fn inspect_candidate(
        &self,
        host: &Arc<WasmHost>,
        staged: &StagedPackage,
        manifest_path: &Path,
        cancellation: &Cancellation,
        command: Option<process::RuntimeCommand>,
    ) -> CapabilityResult<contracts::InspectResult> {
        let request = HostRequest::inspect("import-inspect", &staged.package_hash);
        let response = match command {
            #[cfg(test)]
            Some(command) => {
                host.execute_with_command(&request, manifest_path, cancellation, command)
                    .await?
            }
            #[cfg(test)]
            None => host.execute(&request, manifest_path, cancellation).await?,
            #[cfg(not(test))]
            _ => host.execute(&request, manifest_path, cancellation).await?,
        };
        match response.outcome {
            HostOutcome::Ok(OperationResult::Inspect(result)) => Ok(*result),
            HostOutcome::Error(code) => error(
                code.capability_code(),
                "host rejected the candidate during inspect",
            ),
            _ => error(
                CapabilityErrorCode::ProtocolError,
                "unexpected inspect result",
            ),
        }
    }

    fn validate_inspection(
        &self,
        staged: &StagedPackage,
        inspection: &contracts::InspectResult,
    ) -> CapabilityResult<()> {
        if inspection.contract.validate_subset().is_err() {
            return error(
                CapabilityErrorCode::UnsupportedContract,
                "candidate uses an input contract outside the M1 subset",
            );
        }
        let returned = serde_json::to_value(&inspection.manifest)
            .map_err(|_| storage_error("host manifest is not serialisable"))?;
        if contracts::package_hash(&returned) != staged.package_hash {
            return error(
                CapabilityErrorCode::IntegrityError,
                "host inspection disagrees with the packaged manifest",
            );
        }
        let expected = staged
            .manifest
            .files
            .entries()
            .iter()
            .map(|(role, file)| (*role, file.path.clone(), file.hash.clone()))
            .collect::<Vec<_>>();
        let actual = inspection
            .manifest
            .files
            .entries()
            .iter()
            .map(|(role, file)| (*role, file.path.clone(), file.hash.clone()))
            .collect::<Vec<_>>();
        if expected != actual {
            return error(
                CapabilityErrorCode::IntegrityError,
                "host inspection changed the package file references",
            );
        }
        Ok(())
    }

    pub fn read_revision(&self, revision_id: &str) -> CapabilityResult<repository::RevisionRow> {
        lifecycle::read(&self.writer, |connection| {
            repository::revision_by_id(connection, revision_id)
        })
    }

    pub fn resolve_active(&self, capability_id: &str) -> CapabilityResult<ResolvedCapability> {
        lifecycle::read(&self.writer, |connection| {
            repository::resolve_active(connection, capability_id)?.ok_or_else(|| {
                CapabilityError::new(
                    CapabilityErrorCode::NotActive,
                    "capability has no active revision",
                )
            })
        })
    }

    /// Resolves every allowlisted capability under one catalog read, so a single offer can never
    /// mix revisions from two catalog states. Unknown or inactive ids are skipped; a storage or
    /// integrity failure fails the whole read. No host is started and no lock is held longer than
    /// the read.
    pub fn resolve_publication(
        &self,
        capability_ids: &[String],
    ) -> CapabilityResult<Vec<ResolvedCapability>> {
        lifecycle::read(&self.writer, |connection| {
            let mut resolved = Vec::with_capacity(capability_ids.len());
            for capability_id in capability_ids {
                match repository::resolve_active(connection, capability_id) {
                    Ok(Some(capability)) => resolved.push(capability),
                    Ok(None) => {}
                    Err(error) if error.code == CapabilityErrorCode::Conflict => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(resolved)
        })
    }

    /// Runs the fixed L-Lang verification and then the SAAA acceptance cases.
    pub async fn verify_candidate(
        &self,
        revision_id: &str,
        acceptance_id: &str,
        cancellation: &Cancellation,
    ) -> CapabilityResult<VerificationSummary> {
        let host = self.require_host()?.clone();
        self.ensure_accepting()?;
        let permit = self.acquire_process_slot()?;
        let check_id = crate::new_id("gccheck");
        self.register_execution(&check_id, cancellation);
        let mut guard = ExecutionGuard::new(
            &self.executions,
            &self.writer,
            &check_id,
            ExecutionKind::Check,
        );
        let result = self
            .verify_inner(&host, revision_id, acceptance_id, cancellation, &check_id)
            .await;
        if result.is_ok() {
            guard.settle();
        }
        drop(guard);
        drop(permit);
        result
    }

    async fn verify_inner(
        &self,
        host: &Arc<WasmHost>,
        revision_id: &str,
        acceptance_id: &str,
        cancellation: &Cancellation,
        check_id: &str,
    ) -> CapabilityResult<VerificationSummary> {
        let runtime_digest = host.runtime_digest().to_string();
        let (revision, contract, acceptance) = {
            let _admission = self.admission.lock().await;
            let revision = self.read_revision(revision_id)?;
            if !lifecycle::state_is_verifiable(revision.state) {
                return error(
                    CapabilityErrorCode::NotValidated,
                    "revision state cannot be verified",
                );
            }
            let acceptance = self.ledger.resolve(acceptance_id)?;
            if acceptance.hash != revision.required_acceptance_hash {
                return error(
                    CapabilityErrorCode::IntegrityError,
                    "acceptance reference does not match the revision requirement",
                );
            }
            let inventory = self.store.package_inventory(&revision.package_hash)?;
            if PackageStore::inventory_hash(&inventory) != revision.inventory_hash {
                return error(
                    CapabilityErrorCode::IntegrityError,
                    "managed package contents changed before verification",
                );
            }
            let contract: WasmContract =
                serde_json::from_str(&revision.contract_json).map_err(|_| {
                    CapabilityError::new(
                        CapabilityErrorCode::IntegrityError,
                        "stored contract is unreadable",
                    )
                })?;
            lifecycle::transaction(&self.writer, |transaction| {
                let current = repository::revision_by_id(transaction, revision_id)?;
                if !lifecycle::state_is_verifiable(current.state) {
                    return error(
                        CapabilityErrorCode::NotValidated,
                        "revision state cannot be verified",
                    );
                }
                if repository::running_check_exists(transaction, revision_id)? {
                    return error(
                        CapabilityErrorCode::Busy,
                        "revision verification is already running",
                    );
                }
                repository::insert_check(
                    transaction,
                    check_id,
                    revision_id,
                    &runtime_digest,
                    &revision.inventory_hash,
                    &acceptance.hash,
                    &now_iso(),
                )
            })?;
            (revision, contract, acceptance)
        };

        let manifest_path = self.store.manifest_path(&revision.package_hash);
        let deadline = Instant::now() + limits::ACCEPTANCE_DEADLINE;
        let outcome = verification::run(
            host,
            &manifest_path,
            &revision.package_hash,
            &contract,
            &acceptance,
            cancellation,
            deadline,
        )
        .await;

        let report_ref = match self.save_report(check_id, &outcome.report) {
            Ok(path) => path,
            Err(error) => {
                self.finish_check(check_id, "failed", Some(error.code), None)?;
                return Err(error);
            }
        };
        let final_inventory = self.store.package_inventory(&revision.package_hash);
        let inventory_matches = final_inventory
            .as_ref()
            .map(|inventory| PackageStore::inventory_hash(inventory) == revision.inventory_hash)
            .unwrap_or(false);
        let outcome_passed = outcome.passed && inventory_matches;
        let outcome_code = if inventory_matches {
            outcome.error_code
        } else {
            Some(CapabilityErrorCode::IntegrityError)
        };
        let validated = finalize_check(
            &self.writer,
            revision_id,
            &CheckOutcome {
                check_id: check_id.to_string(),
                passed: outcome_passed,
                error_code: outcome_code,
                runtime_digest,
                hashes: outcome.hashes,
                report_ref: report_ref.clone(),
            },
        )?;
        let error_code = if validated {
            None
        } else if outcome_passed {
            // The revision was stopped or replaced while verification ran: the check still
            // reaches a terminal state, but the revision is never promoted.
            Some(CapabilityErrorCode::NotValidated)
        } else {
            outcome_code
        };
        Ok(VerificationSummary {
            check_id: check_id.to_string(),
            passed: validated,
            error_code,
            detail: outcome.detail,
            report_ref: Some(report_ref),
        })
    }

    fn save_report(&self, check_id: &str, report: &Value) -> CapabilityResult<String> {
        let path = self.store.report_path(check_id);
        let bytes = serde_json::to_vec_pretty(report)
            .map_err(|_| storage_error("verification report is not serialisable"))?;
        std::fs::write(&path, &bytes)
            .map_err(|_| storage_error("verification report could not be stored"))?;
        Ok(contracts::sha256_hex(&bytes))
    }

    /// Runs a short write through the single writer, keeping the capability error code attached.
    fn write<T>(
        &self,
        action: impl FnOnce(&mut rusqlite::Connection) -> CapabilityResult<T>,
    ) -> CapabilityResult<T> {
        self.writer
            .write(|connection| action(connection).map_err(|error| error.encode()))
            .map_err(CapabilityError::decode)
    }

    /// Records a failed call that never reached the detached host task.
    fn fail_call(
        &self,
        call_id: &str,
        status: &str,
        code: CapabilityErrorCode,
    ) -> CapabilityResult<()> {
        let now = now_iso();
        self.write(|connection| {
            repository::finish_call(connection, call_id, status, None, Some(code.as_str()), &now)
        })
    }

    fn finish_check(
        &self,
        check_id: &str,
        status: &str,
        error_code: Option<CapabilityErrorCode>,
        report_ref: Option<&str>,
    ) -> CapabilityResult<()> {
        lifecycle::transaction(&self.writer, |transaction| {
            repository::finish_check(
                transaction,
                check_id,
                status,
                error_code.map(|code| code.as_str()),
                report_ref,
                &now_iso(),
            )
        })
    }

    /// Activation requires a passed check for the *current* runtime and an intact managed copy.
    /// The comparison happens inside the activation transaction, so a revision verified against
    /// an older runtime or a payload changed after verification cannot be promoted.
    pub fn activate_revision(
        &self,
        revision_id: &str,
        expected_epoch: i64,
    ) -> CapabilityResult<repository::Activation> {
        self.require_exposure()?;
        self.ensure_accepting()?;
        let host = self.require_host()?;
        let revision = self.read_revision(revision_id)?;
        let runtime_digest = host.runtime_digest().to_string();
        let inventory_hash =
            PackageStore::inventory_hash(&self.store.package_inventory(&revision.package_hash)?);
        activate_revision(
            &self.writer,
            revision_id,
            expected_epoch,
            &runtime_digest,
            &inventory_hash,
        )
    }

    pub fn suspend_revision(
        &self,
        revision_id: &str,
        expected_epoch: i64,
    ) -> CapabilityResult<repository::CapabilityRow> {
        lifecycle::suspend_revision(&self.writer, revision_id, expected_epoch)
    }

    /// Retires a non-active revision. Package, call history and inspections are retained; an
    /// active revision must be suspended first (plan 4, G04).
    pub fn retire_revision(
        &self,
        revision_id: &str,
        expected_epoch: i64,
    ) -> CapabilityResult<repository::CapabilityRow> {
        lifecycle::retire_revision(&self.writer, revision_id, expected_epoch)
    }

    pub fn catalog_epoch(&self, capability_id: &str) -> CapabilityResult<i64> {
        lifecycle::read(&self.writer, |connection| {
            repository::capability_by_id(connection, capability_id)
                .map(|capability| capability.catalog_epoch)
        })
    }

    pub fn package_manifest(
        &self,
        revision: &repository::RevisionRow,
    ) -> CapabilityResult<PackageManifest> {
        serde_json::from_str(&revision.manifest_json).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::IntegrityError,
                "stored manifest is unreadable",
            )
        })
    }

    /// Accepts a call under a short admission lock, then executes it without holding any lock.
    pub async fn invoke(
        &self,
        request: InvokeRequest,
        cancellation: &Cancellation,
    ) -> CapabilityResult<InvocationResult> {
        self.require_exposure()?;
        self.ensure_accepting()?;
        let host = self.require_host()?.clone();
        let started = Instant::now();
        validate_input(&request.resolved.contract, &request.input)?;
        let permit = self.acquire_process_slot()?;
        let runtime_digest = host.runtime_digest().to_string();

        let accepted = {
            let _admission = self.admission.lock().await;
            // A call queued behind the admission lock must not slip through after a shutdown
            // started: the initial check ran before the lock was acquired.
            self.ensure_accepting()?;
            lifecycle::transaction(&self.writer, |transaction| {
                let capability =
                    repository::capability_by_id(transaction, &request.resolved.capability_id)?;
                if capability.catalog_epoch != request.resolved.catalog_epoch
                    || capability.current_revision_id.as_deref()
                        != Some(request.resolved.revision_id.as_str())
                {
                    return error(
                        CapabilityErrorCode::StaleRevision,
                        "resolved capability revision is no longer current",
                    );
                }
                let revision =
                    repository::revision_by_id(transaction, &request.resolved.revision_id)?;
                if revision.state != RevisionState::Active {
                    return error(
                        CapabilityErrorCode::NotActive,
                        "capability revision is not active",
                    );
                }
                if revision.package_hash != request.resolved.package_hash
                    || revision.contract_hash != request.resolved.contract_hash
                {
                    return error(
                        CapabilityErrorCode::StaleRevision,
                        "resolved capability contract changed",
                    );
                }
                if revision.runtime_digest != runtime_digest {
                    return error(
                        CapabilityErrorCode::NotValidated,
                        "capability must be re-verified for the current runtime",
                    );
                }
                repository::insert_call(
                    transaction,
                    &request.call_id,
                    &revision.id,
                    &revision.package_hash,
                    request.origin,
                    &now_iso(),
                )?;
                if let Some(actor) = &request.actor {
                    super::generation::repository::insert_call_owner_for(
                        transaction,
                        &request.call_id,
                        actor,
                    )?;
                }
                Ok(revision.inventory_hash)
            })
        };
        let inventory_hash = accepted?;

        let integrity = self
            .store
            .package_inventory(&request.resolved.package_hash)
            .map(|inventory| PackageStore::inventory_hash(&inventory) == inventory_hash)
            .unwrap_or(false);
        if !integrity {
            let failure = CapabilityError::new(
                CapabilityErrorCode::IntegrityError,
                "managed package contents changed before invocation",
            );
            self.fail_call(&request.call_id, "failed", failure.code)?;
            return Err(failure);
        }

        let manifest_path = self.store.manifest_path(&request.resolved.package_hash);
        let timeout = request.inner_timeout_ms.min(limits::INNER_TIMEOUT_MAX_MS);
        let host_request = HostRequest::invoke(
            &request.call_id,
            &request.resolved.package_hash,
            request.input.clone(),
            timeout,
        );
        // The host process runs in a detached task that owns the permit and writes the terminal
        // record, so aborting the caller's future neither loses the record nor frees the slot:
        // the task cancels the host if its result receiver disappears, then awaits cleanup.
        // The entry is registered after the last `.await` and handed to that task, so an
        // abandoned caller cannot leak it.
        self.register_execution(&request.call_id, cancellation);
        let (mut sender, receiver) = oneshot::channel();
        let invocation = HostInvocation {
            host,
            writer: self.writer.clone(),
            registry: self.executions.clone(),
            cancellation: cancellation.clone(),
            call_id: request.call_id.clone(),
            revision_id: request.resolved.revision_id.clone(),
            package_hash: request.resolved.package_hash.clone(),
            request: host_request,
            manifest: manifest_path,
            started,
            _permit: Some(permit),
        };
        let abandoned = cancellation.clone();
        tokio::spawn(async move {
            let execution = invocation.run();
            tokio::pin!(execution);
            let outcome = tokio::select! {
                biased;
                _ = sender.closed() => {
                    abandoned.cancel();
                    execution.await
                }
                outcome = &mut execution => outcome,
            };
            let _ = sender.send(outcome);
        });
        match receiver.await {
            Ok(outcome) => outcome,
            Err(_) => error(
                CapabilityErrorCode::StorageError,
                "host invocation result was lost",
            ),
        }
    }

    fn record_import_staging(&self, import_id: &str) -> CapabilityResult<()> {
        let now = now_iso();
        self.write(|connection| repository::insert_import(connection, import_id, &now))
    }

    fn record_import_failure(&self, import_id: &str, error: &CapabilityError) {
        let now = now_iso();
        let _ = self.write(|connection| {
            repository::finish_import(
                connection,
                import_id,
                "failed",
                None,
                Some(error.code.as_str()),
                &now,
            )
        });
    }

    /// Discards staging, records the failure and returns the error, so every import refusal
    /// reports the same way.
    fn refuse_import(
        &self,
        import_id: &str,
        staging: &Path,
        error: CapabilityError,
    ) -> CapabilityResult<RevisionRef> {
        self.store.discard_staging(staging);
        self.record_import_failure(import_id, &error);
        Err(error)
    }
}

fn storage_error(message: &str) -> CapabilityError {
    CapabilityError::new(CapabilityErrorCode::StorageError, message)
}
