//! Generation job orchestration (plan 12.6, C10). Does not live in M1 `CapabilityService`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::builder::CandidateBuild;
use super::config::RegisteredRequest;
use super::contracts::{
    parse_model_response, GenerateInput, GenerationContext, GenerationErrorCode, GenerationReceipt,
    GenerationStatus,
};
use super::generator::Generator;
use super::repository::{self, unix_ms, NewGenerationJob};
use super::recovery;
use crate::generated_capabilities::errors::{CapabilityError, CapabilityErrorCode, CapabilityResult};
use crate::generated_capabilities::inspection::service::InspectionStore;
use crate::generated_capabilities::lifecycle;
use crate::generated_capabilities::publication_sync::{self, PublishRequest};
use crate::generated_capabilities::service::{CapabilityService, ImportCandidate};
use crate::generated_capabilities::host::process::Cancellation;
use crate::new_id;
use crate::persistence::SqliteWriter;
use crate::RunCancellation;

const JOB_DEADLINE: Duration = Duration::from_secs(180);

pub(crate) trait Packager: Send + Sync {
    fn package(&self, capability_id: &str, job_id: &str) -> CapabilityResult<CandidateBuild>;
}

/// Copies a pre-built fixture candidate. Unit tests use this instead of the live L-Lang CLI.
pub(crate) struct FixturePackager {
    pub directory: PathBuf,
}

impl Packager for FixturePackager {
    fn package(&self, _capability_id: &str, _job_id: &str) -> CapabilityResult<CandidateBuild> {
        Ok(CandidateBuild {
            directory: self.directory.clone(),
            package_hash: "fixture".into(),
        })
    }
}

pub(crate) struct SequencePackager {
    pub directories: Vec<PathBuf>,
    index: std::sync::atomic::AtomicUsize,
}

impl SequencePackager {
    pub(crate) fn new(directories: Vec<PathBuf>) -> Self {
        Self {
            directories,
            index: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl Packager for SequencePackager {
    fn package(&self, _capability_id: &str, _job_id: &str) -> CapabilityResult<CandidateBuild> {
        let index = self
            .index
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .min(self.directories.len().saturating_sub(1));
        Ok(CandidateBuild {
            directory: self.directories[index].clone(),
            package_hash: "fixture".into(),
        })
    }
}

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
        match self.generate_inner(context, input, cancellation).await {
            Ok(receipt) => receipt,
            Err(error) => GenerationReceipt {
                job_id: String::new(),
                status: status_for(&error),
                revision_id: None,
                error_code: Some(map_error(&error)),
            },
        }
    }

    async fn generate_inner(
        &self,
        context: GenerationContext,
        input: GenerateInput,
        cancellation: RunCancellation,
    ) -> CapabilityResult<GenerationReceipt> {
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
        let job_id = new_id("gcjob");
        let snapshot = std::fs::read_to_string(&request.request_path).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "registered request is unreadable",
            )
        })?;
        let digest = request.request_hash.clone();
        let expected_epoch = if input.base_revision_id.is_some() {
            self.capabilities
                .catalog_epoch(&request.capability_id)
                .unwrap_or(0)
        } else {
            0
        };
        lifecycle::transaction(&self.writer, |transaction| {
            repository::insert_job(
                transaction,
                &NewGenerationJob {
                    id: job_id.clone(),
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
        self.advance(&job_id, GenerationStatus::Requested, GenerationStatus::Generating)?;
        check_cancel(&cancellation)?;
        let prompt = super::generator::build_prompt(request, snapshot.as_bytes())?;
        let generated = tokio::time::timeout(JOB_DEADLINE, self.generator.generate(&prompt))
            .await
            .map_err(|_| {
                CapabilityError::new(CapabilityErrorCode::Timeout, "generation budget exceeded")
            })?
            .map_err(|code| CapabilityError::new(code.capability_code(), "generator failed"))?;
        let _source = parse_model_response(&generated)?;
        check_cancel(&cancellation)?;
        self.advance(&job_id, GenerationStatus::Generating, GenerationStatus::Building)?;
        let built = self.packager.package(&request.capability_id, &job_id)?;
        check_cancel(&cancellation)?;
        self.advance(&job_id, GenerationStatus::Building, GenerationStatus::Importing)?;
        let imported = self
            .capabilities
            .import_candidate(&ImportCandidate {
                capability_id: Some(request.capability_id.clone()),
                candidate_directory: built.directory,
                acceptance_id: request.acceptance_id.clone(),
                provenance: "generated".to_string(),
            })
            .await?;
        self.advance(&job_id, GenerationStatus::Importing, GenerationStatus::Verifying)?;
        let host_cancel = Cancellation::default();
        if cancellation.is_cancelled() {
            host_cancel.cancel();
        }
        let verified = self
            .capabilities
            .verify_candidate(&imported.revision_id, &request.acceptance_id, &host_cancel)
            .await?;
        if !verified.passed {
            self.fail(&job_id, GenerationStatus::Verifying, GenerationErrorCode::AcceptanceFailed)?;
            return Ok(GenerationReceipt {
                job_id,
                status: GenerationStatus::Failed,
                revision_id: Some(imported.revision_id),
                error_code: Some(GenerationErrorCode::AcceptanceFailed),
            });
        }
        lifecycle::transaction(&self.writer, |transaction| {
            repository::set_revision(transaction, &job_id, &imported.revision_id)
        })?;
        self.advance(
            &job_id,
            GenerationStatus::Verifying,
            GenerationStatus::AwaitingActivation,
        )?;
        if request.auto_activate {
            let epoch = self.capabilities.catalog_epoch(&request.capability_id)?;
            lifecycle::transaction(&self.writer, |transaction| {
                publication_sync::activate_and_publish(
                    transaction,
                    PublishRequest {
                        principal_id: &context.principal_id,
                        revision_id: &imported.revision_id,
                        expected_epoch: epoch,
                        job_id: Some(&job_id),
                        grant_on_create: request.grant_on_create,
                        fail_at: None,
                    },
                )
            })?;
            return Ok(GenerationReceipt {
                job_id,
                status: GenerationStatus::Active,
                revision_id: Some(imported.revision_id),
                error_code: None,
            });
        }
        Ok(GenerationReceipt {
            job_id,
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

    fn fail(
        &self,
        job_id: &str,
        from: GenerationStatus,
        code: GenerationErrorCode,
    ) -> CapabilityResult<()> {
        lifecycle::transaction(&self.writer, |transaction| {
            repository::finish(transaction, job_id, from, GenerationStatus::Failed, Some(code), None, unix_ms())
                .map(|_| ())
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
        _ => GenerationErrorCode::parse(error.code.as_str()).unwrap_or(GenerationErrorCode::Storage),
    }
}

fn status_for(error: &CapabilityError) -> GenerationStatus {
    match map_error(error) {
        GenerationErrorCode::Conflict => GenerationStatus::Conflict,
        GenerationErrorCode::Cancelled => GenerationStatus::Cancelled,
        _ => GenerationStatus::Failed,
    }
}
