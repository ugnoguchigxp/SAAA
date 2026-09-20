//! Fixed contracts for the restricted L-Lang generation flow (plan 12.3).
//!
//! These types are the only place the model-facing JSON shape is defined. Model responses are
//! strict JSON with camelCase keys and unknown fields rejected; principal/project/run/message are
//! never accepted from the model.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::super::contracts::WasmContract;
use super::super::errors::CapabilityError;
use super::super::errors::{error, CapabilityErrorCode, CapabilityResult};

pub const GENERATION_FORMAT_VERSION: u32 = 1;
pub const GENERATION_PROFILE: &str = "predicate-i32-v1";
pub const SOURCE_LANGUAGE: &str = "l-lang";
pub const SOURCE_VERSION: u32 = 1;
/// The model output ceiling from plan 12.3 (bytes of the raw response).
pub const MAX_MODEL_RESPONSE_BYTES: usize = 64 * 1024;

/// Job states. One-to-one with the `status` CHECK in plan 12.4; an unknown value is rejected on
/// deserialization rather than silently mapped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationStatus {
    Requested,
    Generating,
    Building,
    Importing,
    Verifying,
    AwaitingActivation,
    Active,
    Failed,
    Cancelled,
    Conflict,
    Interrupted,
}

impl GenerationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Generating => "generating",
            Self::Building => "building",
            Self::Importing => "importing",
            Self::Verifying => "verifying",
            Self::AwaitingActivation => "awaiting_activation",
            Self::Active => "active",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Conflict => "conflict",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "requested" => Self::Requested,
            "generating" => Self::Generating,
            "building" => Self::Building,
            "importing" => Self::Importing,
            "verifying" => Self::Verifying,
            "awaiting_activation" => Self::AwaitingActivation,
            "active" => Self::Active,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "conflict" => Self::Conflict,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Active | Self::Failed | Self::Cancelled | Self::Conflict | Self::Interrupted
        )
    }

    pub fn is_running(self) -> bool {
        !self.is_terminal()
    }
}

impl Serialize for GenerationStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for GenerationStatus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| serde::de::Error::custom("unknown generation status"))
    }
}

/// Fixed generation error set (plan 12.3). Inspection adds two more codes in its own module.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationErrorCode {
    Unavailable,
    UnsupportedRequest,
    NotAuthorized,
    InvalidInput,
    GenerationContractMismatch,
    ModelError,
    BudgetExceeded,
    BuildError,
    AcceptanceFailed,
    Conflict,
    Cancelled,
    Interrupted,
    Integrity,
    Storage,
}

impl GenerationErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::UnsupportedRequest => "unsupported-request",
            Self::NotAuthorized => "not-authorized",
            Self::InvalidInput => "invalid-input",
            Self::GenerationContractMismatch => "generation-contract-mismatch",
            Self::ModelError => "model-error",
            Self::BudgetExceeded => "budget-exceeded",
            Self::BuildError => "build-error",
            Self::AcceptanceFailed => "acceptance-failed",
            Self::Conflict => "conflict",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
            Self::Integrity => "integrity",
            Self::Storage => "storage",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "unavailable" => Self::Unavailable,
            "unsupported-request" => Self::UnsupportedRequest,
            "not-authorized" => Self::NotAuthorized,
            "invalid-input" => Self::InvalidInput,
            "generation-contract-mismatch" => Self::GenerationContractMismatch,
            "model-error" => Self::ModelError,
            "budget-exceeded" => Self::BudgetExceeded,
            "build-error" => Self::BuildError,
            "acceptance-failed" => Self::AcceptanceFailed,
            "conflict" => Self::Conflict,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            "integrity" => Self::Integrity,
            "storage" => Self::Storage,
            _ => return None,
        })
    }

    pub fn capability_code(self) -> CapabilityErrorCode {
        match self {
            Self::Unavailable => CapabilityErrorCode::Unavailable,
            Self::UnsupportedRequest => CapabilityErrorCode::UnsupportedContract,
            Self::NotAuthorized => CapabilityErrorCode::NotActive,
            Self::InvalidInput => CapabilityErrorCode::InvalidInput,
            Self::GenerationContractMismatch => CapabilityErrorCode::InvalidPackage,
            Self::ModelError => CapabilityErrorCode::ProtocolError,
            Self::BudgetExceeded => CapabilityErrorCode::Timeout,
            Self::Cancelled | Self::Interrupted => CapabilityErrorCode::Cancelled,
            Self::BuildError | Self::AcceptanceFailed => CapabilityErrorCode::VerificationFailed,
            Self::Conflict => CapabilityErrorCode::Conflict,
            Self::Integrity => CapabilityErrorCode::IntegrityError,
            Self::Storage => CapabilityErrorCode::StorageError,
        }
    }
}

impl std::fmt::Display for GenerationErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.as_str())
    }
}

/// Host-supplied caller identity. Never accepted from the model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationContext {
    pub principal_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub input_message_id: String,
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerateInput {
    pub request_id: String,
    pub base_revision_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenerationReceipt {
    pub job_id: String,
    pub status: GenerationStatus,
    pub revision_id: Option<String>,
    pub error_code: Option<GenerationErrorCode>,
}

/// The raw model response: exactly `{"formatVersion":1,"source":{...}}`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelResponse {
    pub format_version: u32,
    pub source: SourceDocument,
}

/// The versioned L-Lang source (JSONC v1) the model may generate. Only `body` is free.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceDocument {
    pub language: String,
    pub version: u32,
    pub id: String,
    pub profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub contract: WasmContract,
    pub body: Value,
}

impl SourceDocument {
    pub fn to_value(&self) -> Result<Value, CapabilityError> {
        serde_json::to_value(self).map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "source is not serialisable",
            )
        })
    }
}

/// Validates the model response against a registered request. Only contract-level mismatches map
/// to `generation-contract-mismatch`; a malformed top-level shape is `invalid-input`.
pub fn validate_model_response(
    response: ModelResponse,
    expected_id: &str,
    expected_fields: &[String],
) -> CapabilityResult<SourceDocument> {
    if response.format_version != GENERATION_FORMAT_VERSION {
        return error(
            GenerationErrorCode::GenerationContractMismatch.capability_code(),
            "unsupported model response format version",
        );
    }
    let source = response.source;
    if source.language != SOURCE_LANGUAGE
        || source.version != SOURCE_VERSION
        || source.profile != GENERATION_PROFILE
    {
        return error(
            GenerationErrorCode::GenerationContractMismatch.capability_code(),
            "model response does not match the fixed source header",
        );
    }
    if source.id != expected_id {
        return error(
            GenerationErrorCode::GenerationContractMismatch.capability_code(),
            "model response id does not match the registered request",
        );
    }
    if let Some(description) = &source.description {
        if description.is_empty() || description.len() > 4096 {
            return error(
                GenerationErrorCode::GenerationContractMismatch.capability_code(),
                "model response description is out of range",
            );
        }
    }
    source.contract.validate_subset().map_err(|_| {
        encode_error(
            GenerationErrorCode::GenerationContractMismatch,
            "model response contract is outside the supported subset",
        )
    })?;
    let actual = source
        .contract
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    let expected = expected_fields
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if actual != expected {
        return error(
            GenerationErrorCode::GenerationContractMismatch.capability_code(),
            "model response contract does not match the registered request",
        );
    }
    if !source.body.is_object() {
        return error(
            GenerationErrorCode::GenerationContractMismatch.capability_code(),
            "model response body must be an object",
        );
    }
    Ok(source)
}

pub fn parse_model_response(text: &str) -> CapabilityResult<SourceDocument> {
    if text.len() > MAX_MODEL_RESPONSE_BYTES {
        return error(
            GenerationErrorCode::BudgetExceeded.capability_code(),
            "model response exceeds the 64 KiB output budget",
        );
    }
    let response: ModelResponse = serde_json::from_str(text).map_err(|_| {
        CapabilityError::new(
            GenerationErrorCode::GenerationContractMismatch.capability_code(),
            "model response is not the strict JSON object",
        )
    })?;
    // Caller re-validates against the request; this only enforces the header shape.
    if response.format_version != GENERATION_FORMAT_VERSION {
        return error(
            GenerationErrorCode::GenerationContractMismatch.capability_code(),
            "unsupported model response format version",
        );
    }
    Ok(response.source)
}

pub fn encode_error(code: GenerationErrorCode, message: &'static str) -> CapabilityError {
    CapabilityError::new(code.capability_code(), message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> Value {
        serde_json::json!({
            "kind": "all",
            "conditions": [{ "kind": "equals", "property": ["enabled"], "value": true }]
        })
    }

    fn response(id: &str, profile: &str, body: Value) -> String {
        serde_json::json!({
            "formatVersion": 1,
            "source": {
                "language": "l-lang",
                "version": 1,
                "id": id,
                "profile": profile,
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
                "body": body
            }
        })
        .to_string()
    }

    #[test]
    fn status_and_error_codes_are_fixed() {
        let statuses = [
            GenerationStatus::Requested,
            GenerationStatus::Generating,
            GenerationStatus::Building,
            GenerationStatus::Importing,
            GenerationStatus::Verifying,
            GenerationStatus::AwaitingActivation,
            GenerationStatus::Active,
            GenerationStatus::Failed,
            GenerationStatus::Cancelled,
            GenerationStatus::Conflict,
            GenerationStatus::Interrupted,
        ];
        let rendered = statuses.map(GenerationStatus::as_str);
        assert_eq!(rendered[0], "requested");
        assert_eq!(rendered[10], "interrupted");
        for (status, text) in statuses.into_iter().zip(rendered) {
            assert_eq!(GenerationStatus::parse(text), Some(status));
        }
        assert!(GenerationStatus::Active.is_terminal());
        assert!(!GenerationStatus::Verifying.is_terminal());

        let codes = [
            GenerationErrorCode::Unavailable,
            GenerationErrorCode::UnsupportedRequest,
            GenerationErrorCode::NotAuthorized,
            GenerationErrorCode::InvalidInput,
            GenerationErrorCode::GenerationContractMismatch,
            GenerationErrorCode::ModelError,
            GenerationErrorCode::BudgetExceeded,
            GenerationErrorCode::BuildError,
            GenerationErrorCode::AcceptanceFailed,
            GenerationErrorCode::Conflict,
            GenerationErrorCode::Cancelled,
            GenerationErrorCode::Interrupted,
            GenerationErrorCode::Integrity,
            GenerationErrorCode::Storage,
        ];
        for code in codes {
            assert_eq!(GenerationErrorCode::parse(code.as_str()), Some(code));
        }
    }

    #[test]
    fn error_to_capability_mapping_keeps_budget_and_cancel_distinct() {
        assert_eq!(
            GenerationErrorCode::BudgetExceeded.capability_code(),
            CapabilityErrorCode::Timeout
        );
        assert_eq!(
            GenerationErrorCode::Cancelled.capability_code(),
            CapabilityErrorCode::Cancelled
        );
        assert_eq!(
            GenerationErrorCode::Conflict.capability_code(),
            CapabilityErrorCode::Conflict
        );
    }

    #[test]
    fn valid_response_passes_and_body_is_free() {
        let source =
            parse_model_response(&response("req-cap", GENERATION_PROFILE, body())).unwrap();
        let validated = validate_model_response(
            ModelResponse {
                format_version: 1,
                source,
            },
            "req-cap",
            &["enabled".to_string()],
        )
        .unwrap();
        assert_eq!(validated.id, "req-cap");
    }

    #[test]
    fn unknown_model_fields_are_rejected() {
        let text = serde_json::json!({
            "formatVersion": 1,
            "source": {
                "language": "l-lang",
                "version": 1,
                "id": "req-cap",
                "profile": "predicate-i32-v1",
                "contract": { "version": 1, "fields": [] },
                "body": {},
                "suite": { "cases": [] }
            }
        })
        .to_string();
        assert!(parse_model_response(&text).is_err());
    }

    #[test]
    fn wrong_id_profile_or_field_order_is_a_contract_mismatch() {
        for text in [
            response("other", GENERATION_PROFILE, body()),
            response("req-cap", "module-value-v1", body()),
            response(
                "req-cap",
                GENERATION_PROFILE,
                serde_json::json!("not-an-object"),
            ),
        ] {
            let source = parse_model_response(&text).unwrap();
            let result = validate_model_response(
                ModelResponse {
                    format_version: 1,
                    source,
                },
                "req-cap",
                &["enabled".to_string()],
            );
            assert!(result.is_err(), "expected mismatch for {text}");
        }
    }

    #[test]
    fn markdown_fences_are_not_repaired() {
        let fenced = format!(
            "```json\n{}\n```",
            response("req-cap", GENERATION_PROFILE, body())
        );
        assert!(parse_model_response(&fenced).is_err());
    }
}
