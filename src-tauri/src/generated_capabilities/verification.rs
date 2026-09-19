use serde::Deserialize;
use serde_json::{Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use super::{
    contracts::{is_hash, HostOutcome, HostRequest, OperationResult, ReportStatus, WasmContract},
    errors::*,
    host::{process::Cancellation, WasmHost},
    limits,
};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptanceIndex {
    format_version: u32,
    entries: Vec<AcceptanceIndexEntry>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptanceIndexEntry {
    id: String,
    file: String,
    hash: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptanceFile {
    version: u32,
    id: String,
    capability_id: String,
    contract: AcceptanceContract,
    cases: Vec<AcceptanceCaseFile>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AcceptanceContract {
    fields: Vec<AcceptanceField>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceField {
    name: String,
    kind: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceCaseFile {
    id: String,
    input: Map<String, Value>,
    expected: bool,
}

#[derive(Clone, Debug)]
pub struct AcceptanceCase {
    pub id: String,
    pub input: Map<String, Value>,
    pub expected: bool,
}

#[derive(Clone, Debug)]
pub struct AcceptanceRef {
    pub id: String,
    pub hash: String,
    pub capability_id: String,
    pub cases: Vec<AcceptanceCase>,
}

/// The fixed acceptance ledger owned by the managing side. Candidates cannot select or supply
/// their own expectations.
#[derive(Clone, Debug)]
pub struct AcceptanceLedger {
    directory: PathBuf,
}

impl AcceptanceLedger {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn resolve(&self, acceptance_id: &str) -> CapabilityResult<AcceptanceRef> {
        let index_bytes = fs::read(self.directory.join("index.json")).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::Unavailable,
                "acceptance ledger is unavailable",
            )
        })?;
        let index: AcceptanceIndex = serde_json::from_slice(&index_bytes).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::Unavailable,
                "acceptance ledger is invalid",
            )
        })?;
        if index.format_version != 1 {
            return error(
                CapabilityErrorCode::Unavailable,
                "unsupported acceptance ledger version",
            );
        }
        let entry = index
            .entries
            .iter()
            .find(|entry| entry.id == acceptance_id)
            .ok_or_else(|| {
                CapabilityError::new(
                    CapabilityErrorCode::Unavailable,
                    "unknown acceptance reference",
                )
            })?;
        if !is_flat_name(&entry.file) || !is_hash(&entry.hash) {
            return error(
                CapabilityErrorCode::Unavailable,
                "invalid acceptance ledger entry",
            );
        }
        let bytes = fs::read(self.directory.join(&entry.file)).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::Unavailable,
                "acceptance case file is unavailable",
            )
        })?;
        if super::contracts::sha256_hex(&bytes) != entry.hash {
            return error(
                CapabilityErrorCode::IntegrityError,
                "acceptance case file hash mismatch",
            );
        }
        let file: AcceptanceFile = serde_json::from_slice(&bytes).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::Unavailable,
                "acceptance case file is invalid",
            )
        })?;
        if file.version != 1
            || file.cases.is_empty()
            || file.cases.len() > limits::MAX_ACCEPTANCE_CASES
        {
            return error(
                CapabilityErrorCode::InvalidPackage,
                "acceptance case file has an unsupported shape",
            );
        }
        if file.contract.fields.is_empty()
            || file
                .contract
                .fields
                .iter()
                .any(|field| field.kind != "boolean" || field.name.is_empty())
        {
            return error(
                CapabilityErrorCode::UnsupportedContract,
                "acceptance cases must use boolean fields only",
            );
        }
        let mut seen = std::collections::HashSet::new();
        let mut cases = Vec::new();
        for case in &file.cases {
            if !seen.insert(case.id.clone()) {
                return error(
                    CapabilityErrorCode::InvalidPackage,
                    "duplicate acceptance case id",
                );
            }
            cases.push(AcceptanceCase {
                id: case.id.clone(),
                input: case.input.clone(),
                expected: case.expected,
            });
        }
        Ok(AcceptanceRef {
            id: file.id,
            hash: entry.hash.clone(),
            capability_id: file.capability_id,
            cases,
        })
    }
}

impl AcceptanceRef {
    /// The acceptance cases must use exactly the revision's contract fields, all boolean.
    pub fn matches_contract(&self, contract: &WasmContract) -> bool {
        let expected = contract
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<std::collections::HashSet<_>>();
        self.cases.iter().all(|case| {
            case.input.len() == expected.len()
                && case.input.iter().all(|(name, value)| {
                    expected.contains(name.as_str()) && value.as_bool().is_some()
                })
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct VerificationHashes {
    pub source_hash: String,
    pub program_hash: String,
    pub artifact_hash: String,
    pub request_revision: String,
    pub suite_hash: String,
}

#[derive(Clone, Debug)]
pub struct VerificationOutcome {
    pub passed: bool,
    pub error_code: Option<CapabilityErrorCode>,
    pub detail: String,
    pub hashes: Option<VerificationHashes>,
    pub report: Value,
}

/// Runs the fixed L-Lang verification, then the SAAA-side independent acceptance cases against
/// the same package. Public `invoke` is deliberately not used: verification must work before a
/// revision is active.
pub async fn run(
    host: &WasmHost,
    package_manifest: &Path,
    package_hash: &str,
    contract: &WasmContract,
    acceptance: &AcceptanceRef,
    cancellation: &Cancellation,
    deadline: Instant,
) -> VerificationOutcome {
    let verify_request = HostRequest::verify("check-verify", package_hash);
    let response = match host
        .execute(&verify_request, package_manifest, cancellation)
        .await
    {
        Ok(response) => response,
        Err(error) => return failure(error.code, "host verification failed", None, Value::Null),
    };
    let report = match response.outcome {
        HostOutcome::Ok(OperationResult::Verify(report)) => report,
        HostOutcome::Error(error) => {
            return failure(
                error.capability_code(),
                "host rejected verification",
                None,
                Value::Null,
            )
        }
        _ => {
            return failure(
                CapabilityErrorCode::ProtocolError,
                "unexpected verify result",
                None,
                Value::Null,
            )
        }
    };
    let report_value = serde_json::to_value(report.as_ref()).unwrap_or(Value::Null);
    let hashes = Some(VerificationHashes {
        source_hash: report.source_hash.clone().unwrap_or_default(),
        program_hash: report.program_hash.clone().unwrap_or_default(),
        artifact_hash: report.artifact_hash.clone().unwrap_or_default(),
        request_revision: report.request_revision.clone().unwrap_or_default(),
        suite_hash: report.suite_hash.clone().unwrap_or_default(),
    });
    if report.status != ReportStatus::Pass {
        return failure(
            CapabilityErrorCode::VerificationFailed,
            "L-Lang verification did not pass",
            hashes,
            report_value,
        );
    }
    if !acceptance.matches_contract(contract) {
        return failure(
            CapabilityErrorCode::InvalidPackage,
            "acceptance cases do not match the revision contract",
            hashes,
            report_value,
        );
    }
    for (index, case) in acceptance.cases.iter().enumerate() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return failure(
                CapabilityErrorCode::Timeout,
                "acceptance cases exceeded their deadline",
                hashes,
                report_value,
            );
        }
        if cancellation.is_cancelled() {
            return failure(
                CapabilityErrorCode::Cancelled,
                "acceptance cases were cancelled",
                hashes,
                report_value,
            );
        }
        let timeout = remaining.min(Duration::from_millis(limits::INNER_TIMEOUT_MAX_MS));
        let timeout = timeout.as_millis().max(1) as u64;
        let request = HostRequest::invoke(
            &format!("accept-{index}"),
            package_hash,
            case.input.clone(),
            timeout,
        );
        let response = match host.execute(&request, package_manifest, cancellation).await {
            Ok(response) => response,
            Err(error) => {
                return failure(
                    error.code,
                    "acceptance invocation failed",
                    hashes,
                    report_value,
                )
            }
        };
        match response.outcome {
            HostOutcome::Ok(OperationResult::Invoke(value)) => {
                if value != case.expected {
                    return failure(
                        CapabilityErrorCode::VerificationFailed,
                        "acceptance case disagreed with the package",
                        hashes,
                        report_value,
                    );
                }
            }
            HostOutcome::Error(error) => {
                return failure(
                    error.capability_code(),
                    "acceptance invocation returned an error",
                    hashes,
                    report_value,
                )
            }
            _ => {
                return failure(
                    CapabilityErrorCode::ProtocolError,
                    "unexpected acceptance result",
                    hashes,
                    report_value,
                )
            }
        }
    }
    VerificationOutcome {
        passed: true,
        error_code: None,
        detail: String::new(),
        hashes,
        report: report_value,
    }
}

fn failure(
    code: CapabilityErrorCode,
    detail: &str,
    hashes: Option<VerificationHashes>,
    report: Value,
) -> VerificationOutcome {
    VerificationOutcome {
        passed: false,
        error_code: Some(code),
        detail: detail.to_string(),
        hashes,
        report,
    }
}

fn is_flat_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && name != "." && name != ".."
}
