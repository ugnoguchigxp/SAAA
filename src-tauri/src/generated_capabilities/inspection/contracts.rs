//! Contracts for execution-time TypeScript inspection (plan 3, 12.3).
//!
//! The inspector is the fixed L-Lang `inspect` CLI run by the trusted generation kit; candidate
//! JavaScript is never used as an entrypoint. The report is parsed strictly and every hash is
//! cross-checked against the managed package before any artifact is stored.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::contracts::{is_hash, WasmContract};
use super::super::errors::{CapabilityError, CapabilityErrorCode};

pub const INSPECTION_FORMAT: &str = "llang-capability-inspection";
pub const INSPECTION_VERSION: u32 = 1;
pub const MAX_INSPECTION_BYTES: u64 = 1024 * 1024;
pub const MAX_TYPESCRIPT_BYTES: usize = 1024 * 1024;

/// Host-supplied inspection identity. `project_id` must match a call recorded with a project.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectionContext {
    pub principal_id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct InspectionReceipt {
    pub inspection_id: String,
    pub revision_id: String,
    pub source_hash: String,
    pub program_hash: String,
    pub artifact_hash: String,
    pub projection_hash: String,
    pub typescript_text: String,
    pub report_json: Value,
    pub comparison_json: Value,
}

impl InspectionReceipt {
    /// The report plus the TypeScript body must fit the fixed 1 MiB budget.
    pub fn within_budget(&self) -> bool {
        self.typescript_text.len() as u64
            + serde_json::to_vec(&self.report_json)
                .map(|bytes| bytes.len() as u64)
                .unwrap_or(u64::MAX)
            <= MAX_INSPECTION_BYTES
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionErrorCode {
    NotGenerated,
    ArtifactMissing,
    NotAuthorized,
    Conflict,
    Integrity,
    Storage,
    Timeout,
    Cancelled,
    Unavailable,
    InvalidInput,
}

impl InspectionErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotGenerated => "not-generated",
            Self::ArtifactMissing => "artifact-missing",
            Self::NotAuthorized => "not-authorized",
            Self::Conflict => "conflict",
            Self::Integrity => "integrity",
            Self::Storage => "storage",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Unavailable => "unavailable",
            Self::InvalidInput => "invalid-input",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "not-generated" => Self::NotGenerated,
            "artifact-missing" => Self::ArtifactMissing,
            "not-authorized" => Self::NotAuthorized,
            "conflict" => Self::Conflict,
            "integrity" => Self::Integrity,
            "storage" => Self::Storage,
            "timeout" => Self::Timeout,
            "cancelled" => Self::Cancelled,
            "unavailable" => Self::Unavailable,
            "invalid-input" => Self::InvalidInput,
            _ => return None,
        })
    }

    pub fn capability_code(self) -> CapabilityErrorCode {
        match self {
            Self::NotGenerated | Self::ArtifactMissing => CapabilityErrorCode::NotValidated,
            Self::NotAuthorized => CapabilityErrorCode::NotActive,
            Self::Conflict => CapabilityErrorCode::Conflict,
            Self::Integrity => CapabilityErrorCode::IntegrityError,
            Self::Storage => CapabilityErrorCode::StorageError,
            Self::Timeout => CapabilityErrorCode::Timeout,
            Self::Cancelled => CapabilityErrorCode::Cancelled,
            Self::Unavailable => CapabilityErrorCode::Unavailable,
            Self::InvalidInput => CapabilityErrorCode::InvalidInput,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InspectionError {
    pub code: InspectionErrorCode,
    pub message: &'static str,
}

impl InspectionError {
    pub fn new(code: InspectionErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }

    pub fn to_capability_error(self) -> CapabilityError {
        CapabilityError::new(self.code.capability_code(), self.message)
    }
}

pub type InspectionResult<T> = Result<T, InspectionError>;

/// The fixed `llang-capability-inspection` report. Unknown fields are rejected so a newer format
/// cannot be silently accepted under an old version.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectionReport {
    pub format: String,
    pub version: u32,
    pub profile: String,
    pub package_hash: String,
    pub metadata: InspectionMetadata,
    pub request: InspectionRequest,
    pub contract: InspectionContract,
    pub artifacts: InspectionArtifacts,
    pub requirement_coverage: RequirementCoverage,
    pub cases: Vec<InspectionCase>,
    pub typescript: InspectionTypescript,
    pub inspection: InspectionLimits,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectionMetadata {
    pub id: String,
    pub release: String,
    pub purpose: String,
    #[serde(rename = "useWhen")]
    pub use_when: String,
    #[serde(rename = "doNotUseWhen")]
    pub do_not_use_when: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectionRequest {
    pub id: String,
    pub body: String,
    pub requirements: Vec<InspectionRequirement>,
    pub request_revision: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectionRequirement {
    pub id: String,
    pub level: String,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectionContract {
    pub input: WasmContract,
    pub output: String,
    pub permissions: Vec<Value>,
    pub input_validation: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectionArtifacts {
    pub source_hash: String,
    pub program_hash: String,
    pub artifact_hash: String,
    pub suite_hash: String,
    pub build: InspectionBuild,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectionBuild {
    pub compiler: String,
    pub backend: String,
    pub options: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequirementCoverage {
    pub coverage: Coverage,
    pub requirements: Vec<CoverageRequirement>,
    pub uncovered_requirements: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Coverage {
    Evaluated,
    NotEvaluated,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoverageRequirement {
    pub id: String,
    pub level: String,
    pub case_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectionCase {
    pub id: String,
    pub requirement_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectionTypescript {
    pub projection_version: u32,
    pub source: String,
    pub projection_hash: String,
    pub source_hash: String,
    pub program_hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectionLimits {
    pub integrity: String,
    pub verification: String,
    pub acceptance: String,
    pub semantic_equivalence: String,
    pub api_calls: u32,
    pub authenticity: String,
    pub coverage_meaning: String,
}

impl InspectionReport {
    /// Structure and fixed markers. Hash matching against the managed package is done separately
    /// by the service, which has the stored revision.
    pub fn validate(&self) -> InspectionResult<()> {
        if self.format != INSPECTION_FORMAT || self.version != INSPECTION_VERSION {
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "unsupported inspection report format",
            ));
        }
        if self.inspection.integrity != "checked"
            || self.inspection.verification != "not-run"
            || self.inspection.acceptance != "not-run"
            || self.inspection.semantic_equivalence != "not-checked"
            || self.inspection.api_calls != 0
        {
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "inspection report does not keep the fixed limitation markers",
            ));
        }
        for hash in [
            &self.package_hash,
            &self.artifacts.source_hash,
            &self.artifacts.program_hash,
            &self.artifacts.artifact_hash,
            &self.artifacts.suite_hash,
            &self.typescript.projection_hash,
            &self.typescript.source_hash,
            &self.typescript.program_hash,
            &self.request.request_revision,
        ] {
            if !is_hash(hash) {
                return Err(InspectionError::new(
                    InspectionErrorCode::Integrity,
                    "inspection report contains an invalid hash",
                ));
            }
        }
        if self.typescript.source.is_empty() || self.typescript.source.len() > MAX_TYPESCRIPT_BYTES
        {
            return Err(InspectionError::new(
                InspectionErrorCode::InvalidInput,
                "inspection TypeScript body is out of range",
            ));
        }
        self.contract.input.validate_subset().map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::Integrity,
                "inspection contract is outside the supported subset",
            )
        })?;
        if self.contract.output != "boolean" || !self.contract.permissions.is_empty() {
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "inspection contract output is not the supported profile",
            ));
        }
        Ok(())
    }

    /// Confirms the report is bound to the exact managed package and stored revision hashes.
    pub fn matches_revision(
        &self,
        package_hash: &str,
        source_hash: &str,
        program_hash: &str,
        artifact_hash: &str,
    ) -> InspectionResult<()> {
        if self.package_hash != package_hash
            || self.artifacts.source_hash != source_hash
            || self.artifacts.program_hash != program_hash
            || self.artifacts.artifact_hash != artifact_hash
            || self.typescript.source_hash != source_hash
            || self.typescript.program_hash != program_hash
        {
            return Err(InspectionError::new(
                InspectionErrorCode::Integrity,
                "inspection report is not bound to the executed revision",
            ));
        }
        Ok(())
    }

    pub fn parse(bytes: &[u8]) -> InspectionResult<Self> {
        if bytes.len() as u64 > MAX_INSPECTION_BYTES {
            return Err(InspectionError::new(
                InspectionErrorCode::InvalidInput,
                "inspection report exceeds 1 MiB",
            ));
        }
        let report: Self = serde_json::from_slice(bytes).map_err(|_| {
            InspectionError::new(
                InspectionErrorCode::Integrity,
                "inspection report is not the fixed JSON shape",
            )
        })?;
        report.validate()?;
        Ok(report)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample_report() -> InspectionReport {
        let ts = "export function evaluate(input: { enabled: boolean }): boolean {\n  return input.enabled;\n}\n";
        serde_json::from_value(serde_json::json!({
            "format": "llang-capability-inspection",
            "version": 1,
            "profile": "predicate-i32-v1",
            "packageHash": "b".repeat(64),
            "metadata": {
                "id": "req-cap",
                "release": "v1",
                "purpose": "p",
                "useWhen": "u",
                "doNotUseWhen": "d"
            },
            "request": {
                "id": "req-cap",
                "body": "b",
                "requirements": [],
                "requestRevision": "c".repeat(64)
            },
            "contract": {
                "input": {
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
                "output": "boolean",
                "permissions": [],
                "inputValidation": "assumed"
            },
            "artifacts": {
                "sourceHash": "d".repeat(64),
                "programHash": "e".repeat(64),
                "artifactHash": "f".repeat(64),
                "suiteHash": "1".repeat(64),
                "build": { "compiler": "c", "backend": "b", "options": "o" }
            },
            "requirementCoverage": {
                "coverage": "not-evaluated",
                "requirements": [],
                "uncoveredRequirements": []
            },
            "cases": [],
            "typescript": {
                "projectionVersion": 1,
                "source": ts,
                "projectionHash": "2".repeat(64),
                "sourceHash": "d".repeat(64),
                "programHash": "e".repeat(64)
            },
            "inspection": {
                "integrity": "checked",
                "verification": "not-run",
                "acceptance": "not-run",
                "semanticEquivalence": "not-checked",
                "apiCalls": 0,
                "authenticity": "a",
                "coverageMeaning": "m"
            }
        }))
        .expect("sample report")
    }

    #[test]
    fn error_codes_round_trip_including_the_inspection_extras() {
        let codes = [
            InspectionErrorCode::NotGenerated,
            InspectionErrorCode::ArtifactMissing,
            InspectionErrorCode::NotAuthorized,
            InspectionErrorCode::Conflict,
            InspectionErrorCode::Integrity,
            InspectionErrorCode::Storage,
            InspectionErrorCode::Timeout,
            InspectionErrorCode::Cancelled,
            InspectionErrorCode::Unavailable,
            InspectionErrorCode::InvalidInput,
        ];
        for code in codes {
            assert_eq!(InspectionErrorCode::parse(code.as_str()), Some(code));
        }
        assert_eq!(
            InspectionErrorCode::Conflict.capability_code(),
            CapabilityErrorCode::Conflict
        );
    }

    #[test]
    fn report_keeps_limitations_and_hashes() {
        let report = sample_report();
        report.validate().unwrap();
        assert!(!report.typescript.source.is_empty());
        let bytes = serde_json::to_vec(&report).unwrap();
        let parsed = InspectionReport::parse(&bytes).unwrap();
        assert_eq!(parsed.package_hash, report.package_hash);
    }

    #[test]
    fn semantic_equivalence_must_stay_not_checked() {
        let mut report = sample_report();
        report.inspection.semantic_equivalence = "proven".into();
        assert!(report.validate().is_err());
    }

    #[test]
    fn unknown_report_fields_are_rejected() {
        let mut value = serde_json::to_value(sample_report()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("extra".into(), true.into());
        assert!(InspectionReport::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn mismatched_revision_is_refused() {
        let report = sample_report();
        assert!(report
            .matches_revision(
                &"b".repeat(64),
                &"d".repeat(64),
                &"e".repeat(64),
                &"f".repeat(64)
            )
            .is_ok());
        assert!(report
            .matches_revision(
                &"a".repeat(64),
                &"d".repeat(64),
                &"e".repeat(64),
                &"f".repeat(64)
            )
            .is_err());
    }
}
