impl CapabilityService {
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
}
impl CapabilityService {
fn save_report(&self, check_id: &str, report: &Value) -> CapabilityResult<String> {
        let path = self.store.report_path(check_id);
        let bytes = serde_json::to_vec_pretty(report)
            .map_err(|_| storage_error("verification report is not serialisable"))?;
        std::fs::write(&path, &bytes)
            .map_err(|_| storage_error("verification report could not be stored"))?;
        Ok(contracts::sha256_hex(&bytes))
    }
}
impl CapabilityService {
/// Runs a short write through the single writer, keeping the capability error code attached.
    fn write<T>(
        &self,
        action: impl FnOnce(&mut rusqlite::Connection) -> CapabilityResult<T>,
    ) -> CapabilityResult<T> {
        self.writer
            .write(|connection| action(connection).map_err(|error| error.encode()))
            .map_err(CapabilityError::decode)
    }
}
impl CapabilityService {
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
}
impl CapabilityService {
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
}
impl CapabilityService {
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
}
impl CapabilityService {
pub fn suspend_revision(
        &self,
        revision_id: &str,
        expected_epoch: i64,
    ) -> CapabilityResult<repository::CapabilityRow> {
        lifecycle::suspend_revision(&self.writer, revision_id, expected_epoch)
    }
}
impl CapabilityService {
/// Retires a non-active revision. Package, call history and inspections are retained; an
    /// active revision must be suspended first (plan 4, G04).
    pub fn retire_revision(
        &self,
        revision_id: &str,
        expected_epoch: i64,
    ) -> CapabilityResult<repository::CapabilityRow> {
        lifecycle::retire_revision(&self.writer, revision_id, expected_epoch)
    }
}
impl CapabilityService {
pub fn catalog_epoch(&self, capability_id: &str) -> CapabilityResult<i64> {
        lifecycle::read(&self.writer, |connection| {
            repository::capability_by_id(connection, capability_id)
                .map(|capability| capability.catalog_epoch)
        })
    }
}
impl CapabilityService {
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
}
impl CapabilityService {
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
}
impl CapabilityService {
fn record_import_staging(&self, import_id: &str) -> CapabilityResult<()> {
        let now = now_iso();
        self.write(|connection| repository::insert_import(connection, import_id, &now))
    }
}
impl CapabilityService {
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
}
impl CapabilityService {
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
