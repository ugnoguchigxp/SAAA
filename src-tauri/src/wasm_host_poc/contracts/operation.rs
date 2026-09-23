use super::*;
pub(crate) const HOST_PROTOCOL: &str = "llang-host-v1";
pub(crate) const MAX_REQUEST_BYTES: usize = 64 * 1024;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Operation {
    Inspect,
    Verify,
    Invoke,
}
impl Operation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::Verify => "verify",
            Self::Invoke => "invoke",
        }
    }
}
#[derive(Clone, Debug)]
pub(crate) struct HostRequest {
    pub(crate) request_id: String,
    pub(crate) operation: Operation,
    pub(crate) package_hash: String,
    pub(crate) input: Option<Map<String, Value>>,
    pub(crate) undefined_fields: Option<Vec<String>>,
    pub(crate) timeout_ms: Option<u64>,
}
impl HostRequest {
    pub(crate) fn inspect(request_id: &str, package_hash: &str) -> Self {
        Self::simple(request_id, package_hash, Operation::Inspect)
    }

    pub(crate) fn verify(request_id: &str, package_hash: &str) -> Self {
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

    pub(crate) fn invoke(
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

    pub(crate) fn to_value(&self) -> Result<Value, String> {
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
pub(crate) enum HostErrorCode {
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
pub(crate) struct HostResponse {
    pub(crate) request_id: String,
    pub(crate) package_hash: Option<String>,
    pub(crate) elapsed_ms: u64,
    pub(crate) outcome: HostOutcome,
}
#[derive(Debug)]
pub(crate) enum HostOutcome {
    Ok(OperationResult),
    Error(HostErrorCode),
}
#[derive(Debug)]
pub(crate) enum OperationResult {
    Inspect(Box<InspectResult>),
    Verify(Box<CapabilityReport>),
    Invoke(bool),
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct InspectResult {
    pub(crate) manifest: CapabilityManifest,
    pub(crate) contract: WasmContract,
    pub(crate) requirements: Vec<SourceRequirement>,
    pub(crate) verification: NotRun,
    pub(crate) acceptance: NotRun,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapabilityManifest {
    pub(crate) version: u8,
    pub(crate) metadata: CapabilityMetadata,
    pub(crate) profile: PredicateProfile,
    pub(crate) output: BooleanOutput,
    pub(crate) permissions: Vec<Value>,
    pub(crate) files: CapabilityFiles,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityMetadata {
    pub(crate) id: String,
    pub(crate) release: String,
    pub(crate) purpose: String,
    pub(crate) use_when: String,
    pub(crate) do_not_use_when: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityFiles {
    pub(crate) source: FileReference,
    pub(crate) lock: FileReference,
    pub(crate) build: FileReference,
    pub(crate) wasm: FileReference,
    pub(crate) tests: FileReference,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileReference {
    pub(crate) path: String,
    pub(crate) hash: String,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WasmContract {
    pub(crate) version: u8,
    pub(crate) fields: Vec<WasmField>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WasmField {
    pub(crate) name: String,
    pub(crate) kind: FieldKind,
    pub(crate) values: Vec<String>,
    pub(crate) nullable: bool,
    pub(crate) undefinable: bool,
    pub(crate) optional: bool,
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
pub(crate) struct SourceRequirement {
    pub(crate) id: String,
    pub(crate) level: RequirementLevel,
    pub(crate) text: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum RequirementLevel {
    Must,
    MustNot,
    Should,
}
#[derive(Debug, Deserialize)]
pub(crate) enum NotRun {
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
    pub(crate) value: bool,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CapabilityReport {
    pub(crate) version: u8,
    pub(crate) verifier: String,
    pub(crate) package_hash: Option<String>,
    pub(crate) status: ReportStatus,
    pub(crate) acceptance: NotRun,
    pub(crate) api_calls: u8,
    pub(crate) results: Vec<CaseResult>,
    pub(crate) requirements: Vec<ReportRequirement>,
    pub(crate) unchecked: Vec<String>,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) errors: usize,
    pub(crate) diagnostics: Vec<String>,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ReportStatus {
    Pass,
    Fail,
    Error,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CaseResult {
    pub(crate) id: String,
    pub(crate) origin: CaseOrigin,
    pub(crate) requirement_ids: Vec<String>,
    pub(crate) expected: CaseObservation,
    pub(crate) actual: CaseObservation,
    pub(crate) status: ReportStatus,
}
#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CaseOrigin {
    Source,
    Suite,
}
#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum CaseObservation {
    Value { value: bool },
    Error { code: String },
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReportRequirement {
    pub(crate) id: String,
    pub(crate) case_ids: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResponseEnvelope {
    pub(crate) protocol: String,
    pub(crate) request_id: Option<String>,
    pub(crate) package_hash: Option<String>,
    pub(crate) elapsed_ms: u64,
    pub(crate) api_calls: u8,
    pub(crate) status: String,
    pub(crate) result: Option<Value>,
    pub(crate) error: Option<String>,
}
pub(crate) fn parse_response(value: Value, request: &HostRequest) -> Result<HostResponse, String> {
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
pub(crate) fn parse_operation_result(
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
    pub(crate) fn validate(&self) -> Result<(), String> {
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
    pub(crate) fn validate(&self) -> Result<(), String> {
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
    pub(crate) fn validate(&self) -> Result<(), String> {
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
