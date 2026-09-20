//! Generation job orchestration (plan 12.6, C10). Does not live in M1 `CapabilityService`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::config::RegisteredRequest;
use super::contracts::{
    parse_model_response, GenerateInput, GenerationContext, GenerationErrorCode, GenerationReceipt,
    GenerationStatus,
};
use super::generator::Generator;
use super::packager::Packager;
use super::recovery;
use super::repository::{self, unix_ms, NewGenerationJob};
use crate::generated_capabilities::errors::{
    CapabilityError, CapabilityErrorCode, CapabilityResult,
};
use crate::generated_capabilities::host::process::Cancellation;
use crate::generated_capabilities::inspection::service::InspectionStore;
use crate::generated_capabilities::lifecycle;
use crate::generated_capabilities::publication_sync::{self, PublishRequest};
use crate::generated_capabilities::service::{CapabilityService, ImportCandidate};
use crate::new_id;
use crate::persistence::SqliteWriter;
use crate::RunCancellation;

#[cfg(not(test))]
const JOB_DEADLINE: Duration = Duration::from_secs(180);
#[cfg(test)]
const JOB_DEADLINE: Duration = Duration::from_millis(250);

#[cfg(test)]
pub(crate) use super::packager::{FixturePackager, SequencePackager};

pub(crate) struct GenerationService {
    writer: Arc<SqliteWriter>,
    capabilities: Arc<CapabilityService>,
    requests: Vec<RegisteredRequest>,
    generator: Arc<dyn Generator>,
    packager: Arc<dyn Packager>,
    inspections: InspectionStore,
}

impl GenerationService {
    pub(crate) fn new(
        writer: Arc<SqliteWriter>,
        capabilities: Arc<CapabilityService>,
        requests: Vec<RegisteredRequest>,
        generator: Arc<dyn Generator>,
        packager: Arc<dyn Packager>,
        data_directory: &Path,
    ) -> Self {
        Self {
            writer,
            capabilities,
            requests,
            generator,
            packager,
            inspections: InspectionStore::open(data_directory),
        }
    }

    pub(crate) fn reconcile(&self) -> CapabilityResult<recovery::GenerationRecoverySummary> {
        recovery::reconcile(&self.writer, &self.inspections)
    }

    pub(crate) async fn generate(
        &self,
        context: GenerationContext,
        input: GenerateInput,
        cancellation: RunCancellation,
    ) -> GenerationReceipt {
        let mut job_id = String::new();
        let mut phase = GenerationStatus::Requested;
        match self
            .generate_inner(context, input, cancellation, &mut job_id, &mut phase)
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                let error_code = map_error(&error);
                if !job_id.is_empty() && error_code != GenerationErrorCode::Conflict {
                    let terminal = status_for(&error);
                    let _ = self.complete(&job_id, phase, terminal, Some(error_code));
                }
                GenerationReceipt {
                    job_id,
                    status: status_for(&error),
                    revision_id: None,
                    error_code: Some(error_code),
                }
            }
        }
    }

    async fn generate_inner(
        &self,
        context: GenerationContext,
        input: GenerateInput,
        cancellation: RunCancellation,
        job_id: &mut String,
        phase: &mut GenerationStatus,
    ) -> CapabilityResult<GenerationReceipt> {
        if context.input_message_id.is_empty() {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidInput,
                "generation identity requires an input message",
            ));
        }
        let request = self
            .requests
            .iter()
            .find(|request| request.id == input.request_id)
            .ok_or_else(|| {
                CapabilityError::new(
                    CapabilityErrorCode::InvalidInput,
                    "unknown registered request",
                )
            })?;
        if input.base_revision_id.is_none() && !request.allow_create {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidInput,
                "create is not allowed for this request",
            ));
        }
        if input.base_revision_id.is_some() && !request.allow_update {
            return Err(CapabilityError::new(
                CapabilityErrorCode::InvalidInput,
                "update is not allowed for this request",
            ));
        }
        let existing = lifecycle::read(&self.writer, |connection| {
            repository::job_by_identity(
                connection,
                &context.principal_id,
                &context.run_id,
                &context.input_message_id,
            )
        })?;
        if let Some(job) = existing {
            return Ok(receipt_from(&job));
        }
        let conflict = lifecycle::read(&self.writer, |connection| {
            repository::active_job_for_capability(connection, &request.capability_id)
        })?;
        if conflict.is_some() {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Conflict,
                "another generation job is running for this capability",
            ));
        }
        let snapshot = std::fs::read_to_string(&request.request_path).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "registered request is unreadable",
            )
        })?;
        let digest = request.request_hash.clone();
        let expected_epoch = match &input.base_revision_id {
            Some(base) => {
                let capability_id = lifecycle::read(&self.writer, |connection| {
                    crate::generated_capabilities::repository::revision_by_id(connection, base)
                        .map(|revision| revision.capability_id)
                })?;
                self.capabilities.catalog_epoch(&capability_id)?
            }
            None => 0,
        };
        let assigned = new_id("gcjob");
        lifecycle::transaction(&self.writer, |transaction| {
            repository::insert_job(
                transaction,
                &NewGenerationJob {
                    id: assigned.clone(),
                    principal_id: context.principal_id.clone(),
                    conversation_id: context.conversation_id.clone(),
                    run_id: context.run_id.clone(),
                    input_message_id: context.input_message_id.clone(),
                    project_id: context.project_id.clone(),
                    request_id: request.id.clone(),
                    request_snapshot_json: snapshot.clone(),
                    request_digest: digest.clone(),
                    capability_id: request.capability_id.clone(),
                    base_revision_id: input.base_revision_id.clone(),
                    expected_epoch,
                    created_at: unix_ms(),
                },
            )
        })?;
        *job_id = assigned;
        *phase = GenerationStatus::Requested;
        self.advance(job_id, *phase, GenerationStatus::Generating)?;
        *phase = GenerationStatus::Generating;
        check_cancel(&cancellation)?;
        let prompt = super::generator::build_prompt(request, snapshot.as_bytes())?;
        let generated = tokio::time::timeout(
            JOB_DEADLINE,
            self.generator.generate(&prompt, &cancellation),
        )
        .await
        .map_err(|_| {
            CapabilityError::new(CapabilityErrorCode::Timeout, "generation budget exceeded")
        })?
        .map_err(|code| CapabilityError::new(code.capability_code(), "generator failed"))?;
        let source = parse_model_response(&generated)?;
        check_cancel(&cancellation)?;
        self.advance(job_id, *phase, GenerationStatus::Building)?;
        *phase = GenerationStatus::Building;
        // Packaging runs the fixed kit (a subprocess, up to the package budget), so it is kept off
        // the async worker threads. The model call and build never hold a DB lock.
        let packager = self.packager.clone();
        let request_for_build = request.clone();
        let job_for_build = job_id.clone();
        let source_for_build = source.clone();
        let built = tokio::task::spawn_blocking(move || {
            packager.package(&request_for_build, &job_for_build, &source_for_build)
        })
        .await
        .map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::Unavailable,
                "the packaging task did not complete",
            )
        })??;
        check_cancel(&cancellation)?;
        self.advance(job_id, *phase, GenerationStatus::Importing)?;
        *phase = GenerationStatus::Importing;
        let imported = self
            .capabilities
            .import_candidate(&ImportCandidate {
                capability_id: None,
                candidate_directory: built.directory,
                acceptance_id: request.acceptance_id.clone(),
                provenance: "generated".to_string(),
            })
            .await?;
        self.advance(job_id, *phase, GenerationStatus::Verifying)?;
        *phase = GenerationStatus::Verifying;
        let host_cancel = Cancellation::default();
        if cancellation.is_cancelled() {
            host_cancel.cancel();
        }
        let verified = self
            .capabilities
            .verify_candidate(&imported.revision_id, &request.acceptance_id, &host_cancel)
            .await?;
        if !verified.passed {
            self.complete(
                job_id,
                *phase,
                GenerationStatus::Failed,
                Some(GenerationErrorCode::AcceptanceFailed),
            )?;
            return Ok(GenerationReceipt {
                job_id: job_id.clone(),
                status: GenerationStatus::Failed,
                revision_id: Some(imported.revision_id),
                error_code: Some(GenerationErrorCode::AcceptanceFailed),
            });
        }
        lifecycle::transaction(&self.writer, |transaction| {
            repository::set_revision(transaction, job_id, &imported.revision_id)
        })?;
        self.advance(job_id, *phase, GenerationStatus::AwaitingActivation)?;
        *phase = GenerationStatus::AwaitingActivation;
        if request.auto_activate {
            // Activation uses the epoch fixed when the job started, so an update that raced another
            // change conflicts instead of publishing on top of it (plan 12.6, G08).
            let job = lifecycle::read(&self.writer, |connection| {
                repository::job_by_id(connection, job_id.as_str())
            })?;
            lifecycle::transaction(&self.writer, |transaction| {
                publication_sync::activate_and_publish(
                    transaction,
                    PublishRequest {
                        principal_id: &context.principal_id,
                        revision_id: &imported.revision_id,
                        expected_epoch: job.expected_epoch,
                        job_id: Some(job_id.as_str()),
                        grant_on_create: request.grant_on_create,
                        fail_at: None,
                    },
                )
            })?;
            return Ok(GenerationReceipt {
                job_id: job_id.clone(),
                status: GenerationStatus::Active,
                revision_id: Some(imported.revision_id),
                error_code: None,
            });
        }
        Ok(GenerationReceipt {
            job_id: job_id.clone(),
            status: GenerationStatus::AwaitingActivation,
            revision_id: Some(imported.revision_id),
            error_code: None,
        })
    }

    fn advance(
        &self,
        job_id: &str,
        from: GenerationStatus,
        to: GenerationStatus,
    ) -> CapabilityResult<()> {
        let moved = lifecycle::transaction(&self.writer, |transaction| {
            repository::cas_status(transaction, job_id, from, to, unix_ms())
        })?;
        if !moved {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Conflict,
                "generation job status changed",
            ));
        }
        Ok(())
    }

    fn complete(
        &self,
        job_id: &str,
        from: GenerationStatus,
        status: GenerationStatus,
        code: Option<GenerationErrorCode>,
    ) -> CapabilityResult<()> {
        lifecycle::transaction(&self.writer, |transaction| {
            repository::finish(transaction, job_id, from, status, code, None, unix_ms()).map(|_| ())
        })
    }
}

fn check_cancel(cancellation: &RunCancellation) -> CapabilityResult<()> {
    if cancellation.is_cancelled() {
        Err(CapabilityError::new(
            CapabilityErrorCode::Cancelled,
            "generation cancelled",
        ))
    } else {
        Ok(())
    }
}

fn receipt_from(job: &repository::GenerationJobRow) -> GenerationReceipt {
    GenerationReceipt {
        job_id: job.id.clone(),
        status: job.status,
        revision_id: job.revision_id.clone(),
        error_code: job
            .error_code
            .as_deref()
            .and_then(GenerationErrorCode::parse),
    }
}

fn map_error(error: &CapabilityError) -> GenerationErrorCode {
    match error.code {
        CapabilityErrorCode::Timeout => GenerationErrorCode::BudgetExceeded,
        CapabilityErrorCode::Cancelled => GenerationErrorCode::Cancelled,
        CapabilityErrorCode::Conflict => GenerationErrorCode::Conflict,
        CapabilityErrorCode::VerificationFailed => GenerationErrorCode::AcceptanceFailed,
        _ => {
            GenerationErrorCode::parse(error.code.as_str()).unwrap_or(GenerationErrorCode::Storage)
        }
    }
}

fn status_for(error: &CapabilityError) -> GenerationStatus {
    match map_error(error) {
        GenerationErrorCode::Conflict => GenerationStatus::Conflict,
        GenerationErrorCode::Cancelled => GenerationStatus::Cancelled,
        _ => GenerationStatus::Failed,
    }
}

#[cfg(test)]
#[path = "../tests/generation_closeout.rs"]
mod generation_closeout;
#[cfg(test)]
#[path = "../tests/generation_fail.rs"]
mod generation_fail;
#[cfg(test)]
#[path = "../tests/generation_flow.rs"]
mod generation_flow;
