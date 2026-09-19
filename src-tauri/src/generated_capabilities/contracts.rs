use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

use super::{errors::*, limits::*};

pub const HOST_PROTOCOL: &str = "llang-host-v1";
pub const VERIFIER_V2: &str = "llang-capability-v2";
pub const PACKAGE_VERSION_V2: u8 = 2;
pub const PROFILE_PREDICATE_I32_V1: &str = "predicate-i32-v1";
pub const CONTRACT_FORMAT: &str = "1";

/// Package roles in the fixed order used for inventories and hashing.
pub const ROLES: [&str; 5] = ["request", "source", "build", "wasm", "tests"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Inspect,
    Verify,
    Invoke,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Verify => "verify",
            Self::Invoke => "invoke",
        }
    }
}

#[derive(Clone, Debug)]
pub struct HostRequest {
    pub request_id: String,
    pub operation: Operation,
    pub package_hash: String,
    pub input: Option<Map<String, Value>>,
    pub timeout_ms: Option<u64>,
}

impl HostRequest {
    pub fn inspect(request_id: &str, package_hash: &str) -> Self {
        Self::simple(request_id, package_hash, Operation::Inspect)
    }

    pub fn verify(request_id: &str, package_hash: &str) -> Self {
        Self::simple(request_id, package_hash, Operation::Verify)
    }

    fn simple(request_id: &str, package_hash: &str, operation: Operation) -> Self {
        Self {
            request_id: request_id.into(),
            operation,
            package_hash: package_hash.into(),
            input: None,
            timeout_ms: None,
        }
    }

    pub fn invoke(
        request_id: &str,
        package_hash: &str,
        input: Map<String, Value>,
        timeout_ms: u64,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            operation: Operation::Invoke,
            package_hash: package_hash.into(),
            input: Some(input),
            timeout_ms: Some(timeout_ms),
        }
    }

    /// `undefinedFields` is fixed to the empty array for the M1 subset.
    pub fn to_value(&self) -> Result<Value, String> {
        if !is_hash(&self.package_hash) || !is_request_id(&self.request_id) {
            return Err("invalid host request identifier".into());
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
            let timeout = self
                .timeout_ms
                .ok_or_else(|| "invoke timeoutMs is missing".to_string())?;
            if timeout == 0 || timeout > INNER_TIMEOUT_MAX_MS {
                return Err("invoke timeoutMs is out of range".into());
            }
            value.insert(
                "input".into(),
                Value::Object(
                    self.input
                        .clone()
                        .ok_or_else(|| "invoke input is missing".to_string())?,
                ),
            );
            value.insert("undefinedFields".into(), Value::Array(Vec::new()));
            value.insert("timeoutMs".into(), Value::from(timeout));
        }
        Ok(Value::Object(value))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostErrorCode {
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

    pub fn capability_code(self) -> CapabilityErrorCode {
        match self {
            Self::InvalidRequest => CapabilityErrorCode::ProtocolError,
            Self::PackageMismatch => CapabilityErrorCode::IntegrityError,
            Self::InvalidInput => CapabilityErrorCode::InvalidInput,
            Self::Timeout => CapabilityErrorCode::Timeout,
            Self::ExecutionError => CapabilityErrorCode::ProtocolError,
        }
    }
}

#[derive(Debug)]
pub struct HostResponse {
    pub request_id: String,
    pub package_hash: Option<String>,
    pub elapsed_ms: u64,
    pub outcome: HostOutcome,
}

#[derive(Debug)]
pub enum HostOutcome {
    Ok(OperationResult),
    Error(HostErrorCode),
}

#[derive(Debug)]
pub enum OperationResult {
    Inspect(Box<InspectResult>),
    Verify(Box<CapabilityReport>),
    Invoke(bool),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectResult {
    pub manifest: PackageManifest,
    pub contract: WasmContract,
    pub requirements: Vec<Requirement>,
    pub verification: NotRun,
    pub acceptance: NotRun,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PackageManifest {
    pub version: u8,
    pub metadata: ManifestMetadata,
    pub profile: String,
    pub output: String,
    pub permissions: Vec<Value>,
    pub files: PackageFiles,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestMetadata {
    pub id: String,
    pub release: String,
    pub purpose: String,
    pub use_when: String,
    pub do_not_use_when: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageFiles {
    pub request: FileRef,
    pub source: FileRef,
    pub build: FileRef,
    pub wasm: FileRef,
    pub tests: FileRef,
}

impl PackageFiles {
    pub fn entries(&self) -> [(&'static str, &FileRef); 5] {
        [
            ("request", &self.request),
            ("source", &self.source),
            ("build", &self.build),
            ("wasm", &self.wasm),
            ("tests", &self.tests),
        ]
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileRef {
    pub path: String,
    pub hash: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WasmContract {
    pub version: u8,
    pub fields: Vec<ContractField>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContractField {
    pub name: String,
    pub kind: FieldKind,
    pub values: Vec<String>,
    pub nullable: bool,
    pub undefinable: bool,
    pub optional: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Boolean,
    Enum,
    String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    pub id: String,
    pub level: RequirementLevel,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequirementLevel {
    Must,
    MustNot,
    Should,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum NotRun {
    #[serde(rename = "not-run")]
    NotRun,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityReport {
    pub version: u8,
    pub verifier: String,
    pub package_hash: Option<String>,
    pub status: ReportStatus,
    pub acceptance: NotRun,
    pub api_calls: u8,
    pub request_revision: Option<String>,
    pub suite_hash: Option<String>,
    pub source_hash: Option<String>,
    pub program_hash: Option<String>,
    pub artifact_hash: Option<String>,
    pub results: Vec<CaseResult>,
    pub requirements: Vec<ReportRequirement>,
    pub uncovered_requirements: Vec<String>,
    pub coverage: Coverage,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportStatus {
    Pass,
    Fail,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Coverage {
    Evaluated,
    NotEvaluated,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaseResult {
    pub id: String,
    pub requirement_ids: Vec<String>,
    pub expected: CaseObservation,
    pub actual: CaseObservation,
    pub status: ReportStatus,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum CaseObservation {
    Value { value: bool },
    Error { code: String },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportRequirement {
    pub id: String,
    pub case_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvokeResult {
    value: bool,
}

pub fn parse_response(value: Value, request: &HostRequest) -> Result<HostResponse, String> {
    let envelope: ResponseEnvelope = serde_json::from_value(value)
        .map_err(|error| format!("invalid response envelope: {error}"))?;
    if envelope.protocol != HOST_PROTOCOL {
        return Err("response protocol mismatch".into());
    }
    if envelope.request_id.as_deref() != Some(request.request_id.as_str()) {
        return Err("response requestId mismatch".into());
    }
    if envelope.api_calls != 0 {
        return Err("response apiCalls must be zero".into());
    }
    let outcome = match envelope.status.as_str() {
        "ok" => {
            if envelope.error.is_some()
                || envelope.package_hash.as_deref() != Some(request.package_hash.as_str())
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
            if let Some(package_hash) = &envelope.package_hash {
                if package_hash != &request.package_hash {
                    return Err("error response packageHash mismatch".into());
                }
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
    pub fn validate(&self) -> Result<(), String> {
        self.manifest.validate()?;
        self.contract.validate_subset()?;
        ensure_unique(
            self.requirements.iter().map(|item| item.id.as_str()),
            "requirement",
        )?;
        if self
            .requirements
            .iter()
            .any(|item| item.text.is_empty() || item.text.len() > 4096)
        {
            return Err("invalid requirement text".into());
        }
        Ok(())
    }
}

impl PackageManifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != PACKAGE_VERSION_V2
            || self.profile != PROFILE_PREDICATE_I32_V1
            || self.output != "boolean"
            || !self.permissions.is_empty()
        {
            return Err("unsupported capability manifest".into());
        }
        self.metadata.validate()?;
        ensure_unique(
            self.files
                .entries()
                .iter()
                .map(|(_, file)| file.path.as_str()),
            "package file",
        )?;
        for (_, file) in self.files.entries() {
            if !safe_flat_path(&file.path) || !is_hash(&file.hash) {
                return Err("invalid package file reference".into());
            }
        }
        Ok(())
    }
}

impl ManifestMetadata {
    pub fn validate(&self) -> Result<(), String> {
        if !is_identifier(&self.id) || !is_identifier(&self.release) {
            return Err("invalid capability metadata identifier".into());
        }
        for text in [&self.purpose, &self.use_when, &self.do_not_use_when] {
            if text.is_empty() || text.len() > MAX_METADATA_TEXT_BYTES {
                return Err("invalid capability metadata text".into());
            }
        }
        Ok(())
    }
}

impl WasmContract {
    /// The M1 input subset: 1..=8 mandatory, non-nullable boolean fields.
    pub fn validate_subset(&self) -> Result<(), String> {
        if self.version != 1
            || self.fields.len() < MIN_CONTRACT_FIELDS
            || self.fields.len() > MAX_CONTRACT_FIELDS
        {
            return Err("unsupported contract field count".into());
        }
        ensure_unique(
            self.fields.iter().map(|field| field.name.as_str()),
            "contract field",
        )?;
        for field in &self.fields {
            if !is_field_name(&field.name) {
                return Err("invalid contract field name".into());
            }
            if field.kind != FieldKind::Boolean
                || !field.values.is_empty()
                || field.nullable
                || field.undefinable
                || field.optional
            {
                return Err("unsupported contract field type".into());
            }
        }
        Ok(())
    }
}

impl CapabilityReport {
    pub fn validate(&self, package_hash: &str) -> Result<(), String> {
        if self.version != PACKAGE_VERSION_V2
            || self.verifier != VERIFIER_V2
            || self.api_calls != 0
            || self.package_hash.as_deref() != Some(package_hash)
        {
            return Err("invalid capability report header".into());
        }
        for hash in [
            &self.request_revision,
            &self.suite_hash,
            &self.source_hash,
            &self.program_hash,
            &self.artifact_hash,
        ] {
            if hash.as_deref().map(|value| !is_hash(value)).unwrap_or(true) {
                return Err("invalid capability report hash".into());
            }
        }
        if self.results.is_empty() {
            return Err("capability report has no cases".into());
        }
        ensure_unique(
            self.results.iter().map(|item| item.id.as_str()),
            "report result",
        )?;
        ensure_unique(
            self.requirements.iter().map(|item| item.id.as_str()),
            "report requirement",
        )?;
        let requirements = self
            .requirements
            .iter()
            .map(|item| (item.id.as_str(), item.case_ids.as_slice()))
            .collect::<std::collections::HashMap<_, _>>();
        for result in &self.results {
            let expected_status = match &result.actual {
                CaseObservation::Error { code } if code != "INVALID_INPUT" => ReportStatus::Error,
                actual if actual == &result.expected => ReportStatus::Pass,
                _ => ReportStatus::Fail,
            };
            if result.status != expected_status {
                return Err("inconsistent case result status".into());
            }
            if result
                .requirement_ids
                .iter()
                .any(|id| !requirements.contains_key(id.as_str()))
            {
                return Err("invalid requirement mapping".into());
            }
        }
        for (requirement, case_ids) in requirements {
            let actual = self
                .results
                .iter()
                .filter(|item| item.requirement_ids.iter().any(|id| id == requirement))
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>();
            if actual != case_ids.iter().map(String::as_str).collect::<Vec<_>>() {
                return Err("inconsistent requirement case mapping".into());
            }
        }
        if self
            .uncovered_requirements
            .iter()
            .any(|id| !self.requirements.iter().any(|item| &item.id == id))
        {
            return Err("unknown uncovered requirement".into());
        }
        let expected = if self.diagnostics.is_empty()
            && !self
                .results
                .iter()
                .any(|item| item.status == ReportStatus::Error)
        {
            if self
                .results
                .iter()
                .any(|item| item.status == ReportStatus::Fail)
                || !self.uncovered_requirements.is_empty()
            {
                ReportStatus::Fail
            } else {
                ReportStatus::Pass
            }
        } else {
            ReportStatus::Error
        };
        if self.status != expected {
            return Err("inconsistent capability report status".into());
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

pub fn is_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn is_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

pub fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    value.len() <= 64
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn is_field_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() || first == b'_' || first == b'$' => {}
        _ => return false,
    }
    value.len() <= 256
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$')
}

/// Flat package file name: no separators, never the manifest itself.
pub fn safe_flat_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 128
        && !path.contains('/')
        && !path.contains('\\')
        && path != "."
        && path != ".."
        && path != "capability.json"
        && !path.starts_with('.')
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// L-Lang's `canonical`: object keys sorted, arrays kept in order, scalars via JSON encoding.
pub fn canonical(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Number(value) => out.push_str(&value.to_string()),
        Value::String(value) => {
            out.push_str(&serde_json::to_string(value).expect("string serializes"));
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("key serializes"));
                out.push(':');
                write_canonical(&map[*key], out);
            }
            out.push('}');
        }
    }
}

/// `package_hash` uses the fixed L-Lang rule: `contentHash(parsed manifest)`.
pub fn package_hash(manifest: &Value) -> String {
    sha256_hex(canonical(manifest).as_bytes())
}

/// `contract_hash` covers only the initial subset: sorted field names, input type, requiredness,
/// profile and output type. Description and release are excluded. Format marker: `contract_format=1`.
pub fn contract_hash(contract: &WasmContract) -> String {
    let mut fields = contract.fields.iter().collect::<Vec<_>>();
    fields.sort_by(|left, right| left.name.cmp(&right.name));
    let mut text = format!(
        "contract_format={CONTRACT_FORMAT}\nprofile={PROFILE_PREDICATE_I32_V1}\noutput=boolean\n"
    );
    for field in fields {
        text.push_str(&format!("field\t{}\tboolean\trequired\n", field.name));
    }
    sha256_hex(text.as_bytes())
}

/// Digest over the fixed-order `(relative name, byte hash)` inventory of the managed copy.
pub fn inventory_hash(entries: &[(String, String)]) -> String {
    let mut text = String::new();
    for (name, hash) in entries {
        text.push_str(name);
        text.push('\u{0}');
        text.push_str(hash);
        text.push('\n');
    }
    sha256_hex(text.as_bytes())
}

#[derive(Clone, Debug)]
pub struct ResolvedCapability {
    pub capability_id: String,
    pub revision_id: String,
    pub package_hash: String,
    pub contract_hash: String,
    pub catalog_epoch: i64,
    pub contract: WasmContract,
}

impl ResolvedCapability {
    pub fn input_fields(&self) -> Vec<String> {
        self.contract
            .fields
            .iter()
            .map(|field| field.name.clone())
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct InvokeRequest {
    pub resolved: ResolvedCapability,
    pub call_id: String,
    pub input: Map<String, Value>,
    pub inner_timeout_ms: u64,
    pub origin: &'static str,
}

impl InvokeRequest {
    pub fn new(resolved: ResolvedCapability, call_id: String, input: Map<String, Value>) -> Self {
        Self {
            resolved,
            call_id,
            input,
            inner_timeout_ms: INNER_TIMEOUT_DEFAULT_MS,
            origin: "internal",
        }
    }
}

#[derive(Clone, Debug)]
pub struct InvocationResult {
    pub call_id: String,
    pub revision_id: String,
    pub package_hash: String,
    pub value: bool,
    pub elapsed_ms: u64,
}
