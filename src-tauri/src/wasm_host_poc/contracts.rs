#![allow(dead_code)]

use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};

pub(super) const HOST_PROTOCOL: &str = "llang-host-v1";
pub(super) const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Operation {
    Inspect,
    Verify,
    Invoke,
}

impl Operation {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Verify => "verify",
            Self::Invoke => "invoke",
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct HostRequest {
    pub(super) request_id: String,
    pub(super) operation: Operation,
    pub(super) package_hash: String,
    pub(super) input: Option<Map<String, Value>>,
    pub(super) undefined_fields: Option<Vec<String>>,
    pub(super) timeout_ms: Option<u64>,
}

impl HostRequest {
    pub(super) fn inspect(request_id: &str, package_hash: &str) -> Self {
        Self::simple(request_id, package_hash, Operation::Inspect)
    }

    pub(super) fn verify(request_id: &str, package_hash: &str) -> Self {
        Self::simple(request_id, package_hash, Operation::Verify)
    }

    fn simple(request_id: &str, package_hash: &str, operation: Operation) -> Self {
        Self {
            request_id: request_id.into(),
            operation,
            package_hash: package_hash.into(),
            input: None,
            undefined_fields: None,
            timeout_ms: None,
        }
    }

    pub(super) fn invoke(
        request_id: &str,
        package_hash: &str,
        input: Map<String, Value>,
        undefined_fields: Vec<String>,
        timeout_ms: u64,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            operation: Operation::Invoke,
            package_hash: package_hash.into(),
            input: Some(input),
            undefined_fields: Some(undefined_fields),
            timeout_ms: Some(timeout_ms),
        }
    }

    pub(super) fn to_value(&self) -> Result<Value, String> {
        if let (Some(input), Some(undefined)) = (&self.input, &self.undefined_fields) {
            if undefined.iter().any(|field| input.contains_key(field)) {
                return Err("undefinedFields conflicts with input".into());
            }
        }
        let mut value = Map::from_iter([
            ("protocol".into(), Value::String(HOST_PROTOCOL.into())),
            ("requestId".into(), Value::String(self.request_id.clone())),
            (
                "operation".into(),
                Value::String(self.operation.as_str().into()),
            ),
            (
                "packageHash".into(),
                Value::String(self.package_hash.clone()),
            ),
        ]);
        if self.operation == Operation::Invoke {
            value.insert(
                "input".into(),
                Value::Object(
                    self.input
                        .clone()
                        .ok_or_else(|| "invoke input is missing".to_string())?,
                ),
            );
            value.insert(
                "undefinedFields".into(),
                serde_json::to_value(
                    self.undefined_fields
                        .as_ref()
                        .ok_or_else(|| "invoke undefinedFields is missing".to_string())?,
                )
                .map_err(|error| error.to_string())?,
            );
            value.insert(
                "timeoutMs".into(),
                Value::from(
                    self.timeout_ms
                        .ok_or_else(|| "invoke timeoutMs is missing".to_string())?,
                ),
            );
        }
        Ok(Value::Object(value))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum HostErrorCode {
    InvalidRequest,
    PackageMismatch,
    InvalidInput,
    Timeout,
    ExecutionError,
}

impl HostErrorCode {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "invalid-request" => Self::InvalidRequest,
            "package-mismatch" => Self::PackageMismatch,
            "invalid-input" => Self::InvalidInput,
            "timeout" => Self::Timeout,
            "execution-error" => Self::ExecutionError,
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub(super) struct HostResponse {
    pub(super) request_id: String,
    pub(super) package_hash: Option<String>,
    pub(super) elapsed_ms: u64,
    pub(super) outcome: HostOutcome,
}

#[derive(Debug)]
pub(super) enum HostOutcome {
    Ok(OperationResult),
    Error(HostErrorCode),
}

#[derive(Debug)]
pub(super) enum OperationResult {
    Inspect(Box<InspectResult>),
    Verify(Box<CapabilityReport>),
    Invoke(bool),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct InspectResult {
    pub(super) manifest: CapabilityManifest,
    pub(super) contract: WasmContract,
    pub(super) requirements: Vec<SourceRequirement>,
    pub(super) verification: NotRun,
    pub(super) acceptance: NotRun,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CapabilityManifest {
    version: u8,
    metadata: CapabilityMetadata,
    profile: PredicateProfile,
    output: BooleanOutput,
    permissions: Vec<Value>,
    files: CapabilityFiles,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityMetadata {
    id: String,
    release: String,
    purpose: String,
    use_when: String,
    do_not_use_when: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityFiles {
    source: FileReference,
    lock: FileReference,
    build: FileReference,
    wasm: FileReference,
    tests: FileReference,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileReference {
    path: String,
    hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WasmContract {
    version: u8,
    fields: Vec<WasmField>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WasmField {
    name: String,
    kind: FieldKind,
    values: Vec<String>,
    nullable: bool,
    undefinable: bool,
    optional: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum FieldKind {
    Boolean,
    Enum,
    String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceRequirement {
    id: String,
    level: RequirementLevel,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RequirementLevel {
    Must,
    MustNot,
    Should,
}

#[derive(Debug, Deserialize)]
pub(super) enum NotRun {
    #[serde(rename = "not-run")]
    Value,
}

#[derive(Debug, Deserialize)]
enum PredicateProfile {
    #[serde(rename = "predicate-i32-v1")]
    Value,
}

#[derive(Debug, Deserialize)]
enum BooleanOutput {
    #[serde(rename = "boolean")]
    Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvokeResult {
    value: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CapabilityReport {
    version: u8,
    verifier: String,
    pub(super) package_hash: Option<String>,
    pub(super) status: ReportStatus,
    acceptance: NotRun,
    api_calls: u8,
    results: Vec<CaseResult>,
    requirements: Vec<ReportRequirement>,
    unchecked: Vec<String>,
    passed: usize,
    failed: usize,
    errors: usize,
    diagnostics: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum ReportStatus {
    Pass,
    Fail,
    Error,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CaseResult {
    id: String,
    origin: CaseOrigin,
    requirement_ids: Vec<String>,
    expected: CaseObservation,
    actual: CaseObservation,
    status: ReportStatus,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum CaseOrigin {
    Source,
    Suite,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum CaseObservation {
    Value { value: bool },
    Error { code: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReportRequirement {
    id: String,
    case_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResponseEnvelope {
    protocol: String,
    request_id: Option<String>,
    package_hash: Option<String>,
    elapsed_ms: u64,
    api_calls: u8,
    status: String,
    result: Option<Value>,
    error: Option<String>,
}

pub(super) fn parse_response(value: Value, request: &HostRequest) -> Result<HostResponse, String> {
    let envelope: ResponseEnvelope = serde_json::from_value(value)
        .map_err(|error| format!("invalid response envelope: {error}"))?;
    if envelope.protocol != HOST_PROTOCOL {
        return Err("response protocol mismatch".into());
    }
    if envelope.request_id.as_deref() != Some(&request.request_id) {
        return Err("response requestId mismatch".into());
    }
    if envelope.api_calls != 0 {
        return Err("response apiCalls must be zero".into());
    }
    let outcome = match envelope.status.as_str() {
        "ok" => {
            if envelope.error.is_some()
                || envelope.package_hash.as_deref() != Some(&request.package_hash)
            {
                return Err("successful response correlation mismatch".into());
            }
            let result = envelope
                .result
                .ok_or_else(|| "successful response has no result".to_string())?;
            HostOutcome::Ok(parse_operation_result(
                result,
                request.operation,
                &request.package_hash,
            )?)
        }
        "error" => {
            if envelope.result.is_some() {
                return Err("error response contains result".into());
            }
            let error = envelope
                .error
                .as_deref()
                .and_then(HostErrorCode::parse)
                .ok_or_else(|| "unknown host error".to_string())?;
            if matches!(error, HostErrorCode::InvalidInput | HostErrorCode::Timeout)
                && request.operation != Operation::Invoke
            {
                return Err("host error is incompatible with operation".into());
            }
            HostOutcome::Error(error)
        }
        _ => return Err("unknown response status".into()),
    };
    Ok(HostResponse {
        request_id: envelope.request_id.unwrap_or_default(),
        package_hash: envelope.package_hash,
        elapsed_ms: envelope.elapsed_ms,
        outcome,
    })
}

fn parse_operation_result(
    value: Value,
    operation: Operation,
    package_hash: &str,
) -> Result<OperationResult, String> {
    match operation {
        Operation::Inspect => {
            let result: InspectResult = serde_json::from_value(value)
                .map_err(|error| format!("invalid inspect result: {error}"))?;
            result.validate()?;
            Ok(OperationResult::Inspect(Box::new(result)))
        }
        Operation::Verify => {
            let result: CapabilityReport = serde_json::from_value(value)
                .map_err(|error| format!("invalid verify result: {error}"))?;
            result.validate(package_hash)?;
            Ok(OperationResult::Verify(Box::new(result)))
        }
        Operation::Invoke => {
            let result: InvokeResult = serde_json::from_value(value)
                .map_err(|error| format!("invalid invoke result: {error}"))?;
            Ok(OperationResult::Invoke(result.value))
        }
    }
}

impl InspectResult {
    fn validate(&self) -> Result<(), String> {
        self.manifest.validate()?;
        self.contract.validate()?;
        ensure_unique(
            self.requirements.iter().map(|item| item.id.as_str()),
            "requirement",
        )?;
        if self.requirements.iter().any(|item| item.text.is_empty()) {
            return Err("empty requirement text".into());
        }
        Ok(())
    }
}

impl CapabilityManifest {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 || !self.permissions.is_empty() {
            return Err("unsupported capability manifest".into());
        }
        if self.metadata.id.is_empty()
            || self.metadata.release.is_empty()
            || self.metadata.purpose.is_empty()
            || self.metadata.use_when.is_empty()
            || self.metadata.do_not_use_when.is_empty()
        {
            return Err("invalid capability metadata".into());
        }
        let files = [
            &self.files.source,
            &self.files.lock,
            &self.files.build,
            &self.files.wasm,
            &self.files.tests,
        ];
        ensure_unique(
            files.iter().map(|file| file.path.as_str()),
            "capability file",
        )?;
        if files
            .iter()
            .any(|file| !safe_flat_path(&file.path) || !is_hash(&file.hash))
        {
            return Err("invalid capability file reference".into());
        }
        Ok(())
    }
}

impl WasmContract {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.fields.is_empty() || self.fields.len() > 64 {
            return Err("invalid Wasm contract".into());
        }
        ensure_unique(
            self.fields.iter().map(|field| field.name.as_str()),
            "contract field",
        )?;
        for field in &self.fields {
            let _nullish = field.nullable || field.undefinable || field.optional;
            match field.kind {
                FieldKind::Enum if field.values.is_empty() => {
                    return Err("enum field has no values".into());
                }
                FieldKind::Boolean | FieldKind::String if !field.values.is_empty() => {
                    return Err("non-enum field has values".into());
                }
                _ => {}
            }
        }
        Ok(())
    }
}

impl CapabilityReport {
    fn validate(&self, package_hash: &str) -> Result<(), String> {
        if self.version != 1
            || self.verifier != "capability-predicate-v1"
            || self.api_calls != 0
            || self.package_hash.as_deref() != Some(package_hash)
            || !self.unchecked.iter().any(|item| item == "SAAA acceptance")
        {
            return Err("invalid capability report header".into());
        }
        ensure_unique(
            self.results.iter().map(|item| item.id.as_str()),
            "report result",
        )?;
        ensure_unique(
            self.requirements.iter().map(|item| item.id.as_str()),
            "report requirement",
        )?;
        let passed = self
            .results
            .iter()
            .filter(|item| item.status == ReportStatus::Pass)
            .count();
        let failed = self
            .results
            .iter()
            .filter(|item| item.status == ReportStatus::Fail)
            .count();
        let errors = self
            .results
            .iter()
            .filter(|item| item.status == ReportStatus::Error)
            .count()
            + self.diagnostics.len();
        let status = if errors > 0 {
            ReportStatus::Error
        } else if failed > 0 {
            ReportStatus::Fail
        } else {
            ReportStatus::Pass
        };
        if (self.passed, self.failed, self.errors, self.status) != (passed, failed, errors, status)
        {
            return Err("inconsistent capability report totals".into());
        }
        let requirements = self
            .requirements
            .iter()
            .map(|item| (item.id.as_str(), item.case_ids.as_slice()))
            .collect::<HashMap<_, _>>();
        for result in &self.results {
            let expected_status = match &result.actual {
                CaseObservation::Error { code } if code != "INVALID_INPUT" => ReportStatus::Error,
                actual if actual == &result.expected => ReportStatus::Pass,
                _ => ReportStatus::Fail,
            };
            if result.status != expected_status {
                return Err("inconsistent case result status".into());
            }
            if result.origin == CaseOrigin::Suite
                && (result.requirement_ids.is_empty()
                    || result
                        .requirement_ids
                        .iter()
                        .any(|id| !requirements.contains_key(id.as_str())))
            {
                return Err("invalid suite requirement mapping".into());
            }
        }
        for (requirement, case_ids) in requirements {
            let actual = self
                .results
                .iter()
                .filter(|item| {
                    item.origin == CaseOrigin::Suite
                        && item.requirement_ids.iter().any(|id| id == requirement)
                })
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>();
            if actual != case_ids.iter().map(String::as_str).collect::<Vec<_>>() {
                return Err("inconsistent requirement case mapping".into());
            }
        }
        Ok(())
    }
}

fn ensure_unique<'a>(values: impl Iterator<Item = &'a str>, label: &str) -> Result<(), String> {
    let mut seen = HashSet::new();
    if values.into_iter().any(|value| !seen.insert(value)) {
        return Err(format!("duplicate {label}"));
    }
    Ok(())
}

fn safe_flat_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 128
        && !path.contains('/')
        && !path.contains('\\')
        && path != "."
        && path != ".."
}

pub(super) fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
