use super::*;
/// How long `shutdown` waits for owned host processes to be reaped.
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(3);
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
/// The single service behind M1 management and execution. Later milestones plug conversation
/// tools and MCP into it; they do not get their own catalog or current pointer.
pub struct CapabilityService {
    pub(super) writer: Arc<SqliteWriter>,
    pub(super) store: PackageStore,
    pub(super) ledger: AcceptanceLedger,
    pub(super) host: Option<Arc<WasmHost>>,
    pub(super) exposure_enabled: bool,
    pub(super) unavailable: Option<CapabilityError>,
    pub(super) admission: AsyncMutex<()>,
    pub(super) process: Arc<Semaphore>,
    pub(super) shutting_down: AtomicBool,
    pub(super) executions: ExecutionRegistry,
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
}
impl CapabilityService {
    pub(crate) fn writer(&self) -> &Arc<SqliteWriter> {
        &self.writer
    }
}
impl CapabilityService {
    pub fn store(&self) -> &PackageStore {
        &self.store
    }
}
impl CapabilityService {
    pub fn is_ready(&self) -> bool {
        self.host.is_some()
    }
}
impl CapabilityService {
    pub fn is_exposed(&self) -> bool {
        self.exposure_enabled && self.host.is_some()
    }
}
impl CapabilityService {
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
}
impl CapabilityService {
    pub(super) fn unsettled_rows(&self) -> i64 {
        lifecycle::read(&self.writer, repository::unsettled_rows).unwrap_or(i64::MAX)
    }
}
impl CapabilityService {
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst)
    }
}
impl CapabilityService {
    pub fn unavailable_reason(&self) -> Option<&CapabilityError> {
        self.unavailable.as_ref()
    }
}
impl CapabilityService {
    pub(crate) fn ensure_accepting(&self) -> CapabilityResult<()> {
        if self.is_shutting_down() {
            return error(
                CapabilityErrorCode::Unavailable,
                "the capability service is shutting down",
            );
        }
        Ok(())
    }
}
impl CapabilityService {
    pub(super) fn register_execution(&self, id: &str, cancellation: &Cancellation) {
        execution::register(&self.executions, id, cancellation);
    }
}
impl CapabilityService {
    pub(crate) fn require_host(&self) -> CapabilityResult<&Arc<WasmHost>> {
        match (&self.host, &self.unavailable) {
            (Some(host), _) => Ok(host),
            (None, Some(error)) => Err(error.clone()),
            (None, None) => error(
                CapabilityErrorCode::Disabled,
                "generated capabilities are disabled",
            ),
        }
    }
}
impl CapabilityService {
    pub(super) fn require_exposure(&self) -> CapabilityResult<()> {
        if self.exposure_enabled {
            Ok(())
        } else {
            error(
                CapabilityErrorCode::Disabled,
                "generated capability exposure is disabled",
            )
        }
    }
}
impl CapabilityService {
    pub(crate) fn acquire_process_slot(&self) -> CapabilityResult<OwnedSemaphorePermit> {
        self.process.clone().try_acquire_owned().map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::Busy,
                "another host process is already running",
            )
        })
    }
}
impl CapabilityService {
    /// Staging, size/path checks, trusted inspect, publish and catalog insert.
    pub async fn import_candidate(
        &self,
        candidate: &ImportCandidate,
    ) -> CapabilityResult<RevisionRef> {
        self.import_candidate_with(candidate, None).await
    }
}
impl CapabilityService {
    pub(super) async fn import_candidate_with(
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
}
impl CapabilityService {
    pub(super) async fn import_candidate_inner(
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
}
impl CapabilityService {
    pub(super) async fn inspect_candidate(
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
}
impl CapabilityService {
    pub(super) fn validate_inspection(
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
}
impl CapabilityService {
    pub fn read_revision(&self, revision_id: &str) -> CapabilityResult<repository::RevisionRow> {
        lifecycle::read(&self.writer, |connection| {
            repository::revision_by_id(connection, revision_id)
        })
    }
}
impl CapabilityService {
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
}
impl CapabilityService {
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
}
impl CapabilityService {
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
}
