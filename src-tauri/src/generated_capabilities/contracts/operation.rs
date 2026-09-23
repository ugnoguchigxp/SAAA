use super::*;
pub const HOST_PROTOCOL: &str = "llang-host-v1";
pub const VERIFIER_V2: &str = "llang-capability-v2";
pub const PACKAGE_VERSION_V2: u8 = 2;
pub const PROFILE_PREDICATE_I32_V1: &str = "predicate-i32-v1";
pub const CONTRACT_FORMAT: &str = "1";
/// Package roles in the fixed order used for inventories and hashing.
pub const ROLES: [&str;
5] = ["request", "source", "build", "wasm", "tests"];
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
    pub(super) protocol: String,
    pub(super) request_id: Option<String>,
    pub(super) package_hash: Option<String>,
    pub(super) elapsed_ms: u64,
    pub(super) api_calls: u8,
    pub(super) status: String,
    pub(super) result: Option<Value>,
    pub(super) error: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvokeResult {
    pub(super) value: bool,
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
pub(super) fn parse_operation_result(
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
