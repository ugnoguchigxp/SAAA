//! Fixed L-Lang build/inspection bridge (plan 12.2, C09/C05).
//!
//! The host copies the registered request/suite/metadata into the job workspace, writes the model
//! source, and runs the fixed kit. The model never supplies a path, command or release id.

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::super::contracts::sha256_hex;
use super::super::errors::{CapabilityError, CapabilityErrorCode, CapabilityResult};
use super::config::RegisteredRequest;
use super::contracts::{encode_error, GenerationErrorCode, SourceDocument};
use super::kit::{GenerationKit, INSPECT_TIMEOUT, PACKAGE_TIMEOUT};
use crate::generated_capabilities::inspection::contracts::{
    InspectionError, InspectionErrorCode, InspectionReport, InspectionResult,
};
use crate::generated_capabilities::inspection::service::Inspector;

pub const SOURCE_FILE: &str = "source.llang.jsonc";
pub const REQUEST_FILE: &str = "request.json";
pub const SUITE_FILE: &str = "tests.json";
pub const METADATA_FILE: &str = "metadata.json";
pub const CANDIDATE_DIR: &str = "candidate";

#[derive(Clone, Debug)]
pub struct CandidateBuild {
    pub directory: PathBuf,
    pub package_hash: String,
}

/// Copies the registered request/suite/metadata into the job workspace after re-checking each file
/// against the hash fixed at load time. A later edit to a registered file is therefore refused, not
/// silently built, and the running job never re-reads the original path.
pub fn stage_registered_files(
    workspace: &Path,
    request: &RegisteredRequest,
) -> CapabilityResult<()> {
    fs::create_dir_all(workspace).map_err(|_| {
        encode_error(
            GenerationErrorCode::Storage,
            "could not create the generation workspace",
        )
    })?;
    for (name, path, expected) in [
        (REQUEST_FILE, &request.request_path, &request.request_hash),
        (SUITE_FILE, &request.suite_path, &request.suite_hash),
        (
            METADATA_FILE,
            &request.metadata_path,
            &request.metadata_hash,
        ),
    ] {
        let bytes = fs::read(path).map_err(|_| {
            encode_error(
                GenerationErrorCode::Storage,
                "a registered file is unreadable",
            )
        })?;
        if sha256_hex(&bytes) != *expected {
            return Err(encode_error(
                GenerationErrorCode::Integrity,
                "a registered file changed since it was loaded",
            ));
        }
        fs::write(workspace.join(name), &bytes).map_err(|_| {
            encode_error(
                GenerationErrorCode::Storage,
                "could not stage a registered file",
            )
        })?;
    }
    Ok(())
}

/// Applies the fixed metadata identity: `id` is the registered capability id and `release` is
/// host-generated from the job id. Every other field is copied from the registered metadata.
pub fn prepare_metadata(
    registered: &[u8],
    capability_id: &str,
    job_id: &str,
) -> CapabilityResult<Vec<u8>> {
    let mut value: Value = serde_json::from_slice(registered).map_err(|_| {
        encode_error(
            GenerationErrorCode::InvalidInput,
            "the registered metadata is not valid JSON",
        )
    })?;
    let object = value.as_object_mut().ok_or_else(|| {
        encode_error(
            GenerationErrorCode::InvalidInput,
            "the registered metadata is not an object",
        )
    })?;
    object.insert("id".into(), Value::String(capability_id.to_string()));
    object.insert("release".into(), Value::String(format!("gen-{job_id}")));
    serde_json::to_vec(&value).map_err(|_| {
        encode_error(
            GenerationErrorCode::Storage,
            "the metadata could not be serialised",
        )
    })
}

pub fn write_source(workspace: &Path, source: &SourceDocument) -> CapabilityResult<PathBuf> {
    let bytes = serde_json::to_vec_pretty(source).map_err(|_| {
        encode_error(
            GenerationErrorCode::Storage,
            "the source could not be serialised",
        )
    })?;
    let path = workspace.join(SOURCE_FILE);
    fs::write(&path, bytes).map_err(|_| {
        encode_error(
            GenerationErrorCode::Storage,
            "the source could not be written",
        )
    })?;
    Ok(path)
}

/// Runs the fixed `package` command and returns the built candidate directory and package hash.
pub fn build_candidate(kit: &GenerationKit, workspace: &Path) -> CapabilityResult<CandidateBuild> {
    let source = workspace.join(SOURCE_FILE);
    let candidate = workspace.join(CANDIDATE_DIR);
    if candidate.exists() {
        return Err(encode_error(
            GenerationErrorCode::Storage,
            "the candidate output directory already exists",
        ));
    }
    let output = kit.run(
        &[
            "package",
            &source.to_string_lossy(),
            "--request",
            &workspace.join(REQUEST_FILE).to_string_lossy(),
            "--suite",
            &workspace.join(SUITE_FILE).to_string_lossy(),
            "--metadata",
            &workspace.join(METADATA_FILE).to_string_lossy(),
            "--out-dir",
            &candidate.to_string_lossy(),
        ],
        PACKAGE_TIMEOUT,
        workspace,
    )?;
    if !output.status.success() {
        return Err(encode_error(
            GenerationErrorCode::BuildError,
            "the fixed package command rejected the source",
        ));
    }
    let parsed: Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        encode_error(
            GenerationErrorCode::BuildError,
            "the package command did not return the fixed JSON",
        )
    })?;
    let package_hash = parsed
        .get("packageHash")
        .and_then(Value::as_str)
        .filter(|value| value.len() == 64)
        .ok_or_else(|| {
            encode_error(
                GenerationErrorCode::BuildError,
                "the package command returned no package hash",
            )
        })?
        .to_string();
    Ok(CandidateBuild {
        directory: candidate,
        package_hash,
    })
}

/// Trusted inspector backed by the same kit (`inspect` command). The candidate's own JavaScript is
/// never executed; the kit reads the package and returns the fixed report.
pub struct KitInspector {
    pub(super) kit: std::sync::Arc<GenerationKit>,
    pub(super) workspace_root: PathBuf,
}

impl KitInspector {
    pub fn new(kit: std::sync::Arc<GenerationKit>, workspace_root: PathBuf) -> Self {
        Self {
            kit,
            workspace_root,
        }
    }
}

impl Inspector for KitInspector {
    fn inspect(
        &self,
        package_directory: &Path,
        package_hash: &str,
    ) -> InspectionResult<InspectionReport> {
        // The kit's `inspect --out-dir` requires a **new** directory, so only the parent is created
        // here; the CLI creates the leaf and we remove it after reading the report.
        fs::create_dir_all(&self.workspace_root).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::Storage,
                "could not create the inspection workspace",
            )
        })?;
        let directory = self
            .workspace_root
            .join(format!("inspection-{}", uuid::Uuid::new_v4().simple()));
        let manifest = package_directory.join("capability.json");
        let output = match self.kit.run(
            &[
                "inspect",
                &manifest.to_string_lossy(),
                "--out-dir",
                &directory.to_string_lossy(),
            ],
            INSPECT_TIMEOUT,
            &self.workspace_root,
        ) {
            Ok(output) => output,
            Err(error) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(map_kit_run_error(error));
            }
        };
        if !output.status.success() {
            let _ = fs::remove_dir_all(&directory);
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "the fixed inspector rejected the package",
            ));
        }
        let report_path = directory.join("inspection.json");
        let bytes = fs::read(&report_path).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::ArtifactMissing,
                "the inspector did not publish a report",
            )
        });
        let _ = fs::remove_dir_all(&directory);
        let bytes = bytes?;
        let report = InspectionReport::parse(&bytes)?;
        if report.package_hash != package_hash {
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "the inspector report does not match the package",
            ));
        }
        Ok(report)
    }
}

fn map_kit_run_error(error: CapabilityError) -> InspectionError {
    let code = match error.code {
        CapabilityErrorCode::Timeout => InspectionErrorCode::Timeout,
        CapabilityErrorCode::Cancelled => InspectionErrorCode::Cancelled,
        CapabilityErrorCode::Unavailable | CapabilityErrorCode::Disabled => {
            InspectionErrorCode::Unavailable
        }
        _ => InspectionErrorCode::Integrity,
    };
    InspectionError::new(code, "the fixed inspector could not complete")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated_capabilities::generation::config::{RegisteredRequest, RequestScope};

    fn write(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).unwrap();
    }

    fn registered(dir: &Path) -> RegisteredRequest {
        let request = dir.join("request.json");
        let suite = dir.join("tests.json");
        let metadata = dir.join("metadata.json");
        write(&request, br#"{"version":2,"id":"req-cap","body":"x"}"#);
        write(&suite, br#"{"version":2}"#);
        write(
            &metadata,
            br#"{"id":"other","release":"old","purpose":"p"}"#,
        );
        RegisteredRequest {
            id: "req-1".into(),
            capability_id: "req-cap".into(),
            purpose: "p".into(),
            fields: vec!["enabled".into()],
            request_hash: sha256_hex(&fs::read(&request).unwrap()),
            request_path: request,
            suite_hash: sha256_hex(&fs::read(&suite).unwrap()),
            suite_path: suite,
            metadata_hash: sha256_hex(&fs::read(&metadata).unwrap()),
            metadata_path: metadata,
            acceptance_id: "acc-1".into(),
            scope: RequestScope::User,
            allow_create: true,
            allow_update: false,
            auto_activate: true,
            grant_on_create: true,
        }
    }

    #[test]
    fn staged_files_are_rechecked_against_the_load_time_hash() {
        let dir = tempfile::tempdir().unwrap();
        let request = registered(dir.path());
        let workspace = dir.path().join("workspace");
        stage_registered_files(&workspace, &request).unwrap();
        assert!(workspace.join(REQUEST_FILE).is_file());

        // An edited registered file is refused rather than silently built.
        write(
            &request.request_path,
            br#"{"version":2,"id":"req-cap","body":"changed"}"#,
        );
        let error = stage_registered_files(&workspace, &request).unwrap_err();
        assert_eq!(
            error.code,
            crate::generated_capabilities::errors::CapabilityErrorCode::IntegrityError
        );
    }

    #[test]
    fn metadata_identity_is_host_generated() {
        let registered_metadata =
            br#"{"id":"other","release":"old","purpose":"p","useWhen":"u","doNotUseWhen":"d"}"#;
        let bytes = prepare_metadata(registered_metadata, "req-cap", "job-9").unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["id"], "req-cap");
        assert_eq!(value["release"], "gen-job-9");
        assert_eq!(value["purpose"], "p");
    }

    #[test]
    fn write_source_uses_the_fixed_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let source = crate::generated_capabilities::generation::contracts::parse_model_response(
            &serde_json::json!({
                "formatVersion": 1,
                "source": {
                    "language": "l-lang",
                    "version": 1,
                    "id": "req-cap",
                    "profile": "predicate-i32-v1",
                    "contract": {
                        "version": 1,
                        "fields": [{
                            "name": "enabled",
                            "kind": "boolean",
                            "values": [],
                            "nullable": false,
                            "undefinable": false,
                            "optional": false
                        }]
                    },
                    "body": { "kind": "all", "conditions": [] }
                }
            })
            .to_string(),
        )
        .unwrap();
        let path = write_source(dir.path(), &source).unwrap();
        assert_eq!(path.file_name().unwrap(), SOURCE_FILE);
    }
}
