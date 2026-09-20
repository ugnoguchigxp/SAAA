//! Staging and kit packaging for a generation job. The model never supplies a path or command.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::builder::{self, CandidateBuild};
use super::config::RegisteredRequest;
use super::contracts::SourceDocument;
use super::generator::{ConversationProviderGenerator, DisabledGenerator};
use super::kit::GenerationKit;
use crate::generated_capabilities::errors::{
    CapabilityError, CapabilityErrorCode, CapabilityResult,
};
use crate::persistence::SqliteWriter;

pub(crate) fn production_runtime(
    writer: Arc<SqliteWriter>,
    data_directory: &Path,
) -> (
    Vec<RegisteredRequest>,
    Arc<dyn super::generator::Generator>,
    Arc<dyn Packager>,
) {
    let disabled: (
        Vec<RegisteredRequest>,
        Arc<dyn super::generator::Generator>,
        Arc<dyn Packager>,
    ) = (
        Vec::new(),
        Arc::new(DisabledGenerator),
        Arc::new(UnavailablePackager),
    );
    match super::config::from_environment() {
        Ok(Some((config, requests))) => match GenerationKit::load(&config) {
            Ok(kit) => (
                requests,
                Arc::new(ConversationProviderGenerator::new(writer)),
                Arc::new(KitPackager::new(Arc::new(kit), data_directory)),
            ),
            Err(error) => {
                eprintln!("generated capability generation kit skipped: {error}");
                disabled
            }
        },
        Ok(None) => disabled,
        Err(error) => {
            eprintln!("generated capability generation config skipped: {error}");
            disabled
        }
    }
}

pub(crate) const GENERATION_WORKSPACE: &str = "generated-generation";

pub(crate) trait Packager: Send + Sync {
    fn package(
        &self,
        request: &RegisteredRequest,
        job_id: &str,
        source: &SourceDocument,
    ) -> CapabilityResult<CandidateBuild>;
}

pub(crate) struct KitPackager {
    kit: Arc<GenerationKit>,
    workspace_root: PathBuf,
}

impl KitPackager {
    pub(crate) fn new(kit: Arc<GenerationKit>, data_directory: &Path) -> Self {
        Self {
            kit,
            workspace_root: data_directory.join(GENERATION_WORKSPACE),
        }
    }
}

impl Packager for KitPackager {
    fn package(
        &self,
        request: &RegisteredRequest,
        job_id: &str,
        source: &SourceDocument,
    ) -> CapabilityResult<CandidateBuild> {
        let workspace = self.workspace_root.join(job_id);
        if workspace.exists() {
            return Err(CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "the generation workspace already exists",
            ));
        }
        builder::stage_registered_files(&workspace, request)?;
        let registered_metadata = std::fs::read(&request.metadata_path).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "the registered metadata is unreadable",
            )
        })?;
        let metadata =
            builder::prepare_metadata(&registered_metadata, &request.capability_id, job_id)?;
        std::fs::write(workspace.join(builder::METADATA_FILE), &metadata).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "the generation metadata could not be written",
            )
        })?;
        builder::write_source(&workspace, source)?;
        builder::build_candidate(&self.kit, &workspace)
    }
}

pub(crate) struct UnavailablePackager;

impl Packager for UnavailablePackager {
    fn package(
        &self,
        _request: &RegisteredRequest,
        _job_id: &str,
        _source: &SourceDocument,
    ) -> CapabilityResult<CandidateBuild> {
        Err(CapabilityError::new(
            CapabilityErrorCode::Unavailable,
            "generation packaging is not configured",
        ))
    }
}

#[cfg(test)]
pub(crate) struct FixturePackager {
    pub directory: PathBuf,
}

#[cfg(test)]
impl Packager for FixturePackager {
    fn package(
        &self,
        _request: &RegisteredRequest,
        _job_id: &str,
        _source: &SourceDocument,
    ) -> CapabilityResult<CandidateBuild> {
        Ok(CandidateBuild {
            directory: self.directory.clone(),
            package_hash: "fixture".into(),
        })
    }
}

#[cfg(test)]
pub(crate) struct SequencePackager {
    pub directories: Vec<PathBuf>,
    index: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl SequencePackager {
    pub(crate) fn new(directories: Vec<PathBuf>) -> Self {
        Self {
            directories,
            index: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[cfg(test)]
impl Packager for SequencePackager {
    fn package(
        &self,
        _request: &RegisteredRequest,
        _job_id: &str,
        _source: &SourceDocument,
    ) -> CapabilityResult<CandidateBuild> {
        if self.directories.is_empty() {
            return Err(CapabilityError::new(
                CapabilityErrorCode::Unavailable,
                "no fixture candidates",
            ));
        }
        let index = self
            .index
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .min(self.directories.len() - 1);
        Ok(CandidateBuild {
            directory: self.directories[index].clone(),
            package_hash: "fixture".into(),
        })
    }
}
