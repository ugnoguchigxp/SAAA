//! One-call model generation of a versioned L-Lang source (plan 12.3, C09).
//!
//! The host builds the prompt from a registered request. The independent acceptance truth table is
//! never included. The model response is strict JSON and is validated against the registered
//! contract by `contracts::validate_model_response`.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::super::errors::CapabilityResult;
use super::config::RegisteredRequest;
use super::contracts::{encode_error, GenerationErrorCode};

pub const GENERATION_SYSTEM_PROMPT: &str =
    "You write one L-Lang predicate source for a capability. \
Return only the strict JSON object {\"formatVersion\":1,\"source\":{...L-Lang Source v1...}}. The \
source header (language, version, id, profile, contract) must match the request exactly; only \
source.body is yours to write. Never return tests, metadata, paths, commands, markdown or prose. \
Do not claim the capability is verified.";

#[derive(Clone, Debug)]
pub struct GenerationPrompt {
    pub system: String,
    pub user: String,
    pub request_id: String,
    pub capability_id: String,
    pub fields: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FakeBody {
    /// `field0 && !field1` — the plan's request A.
    EnabledAndNotSuspended,
    /// `field0` — the plan's change request B.
    EnabledOnly,
    /// A deliberately wrong predicate for negative tests.
    Wrong,
}

/// Builds the prompt from the registered request's own bytes. The request body and contract are
/// included; nothing from the independent acceptance is.
pub fn build_prompt(
    request: &RegisteredRequest,
    request_bytes: &[u8],
) -> CapabilityResult<GenerationPrompt> {
    let value: Value = serde_json::from_slice(request_bytes).map_err(|_| {
        encode_error(
            GenerationErrorCode::InvalidInput,
            "the registered request is not valid JSON",
        )
    })?;
    let body = value.get("body").and_then(Value::as_str).ok_or_else(|| {
        encode_error(
            GenerationErrorCode::InvalidInput,
            "the registered request has no body",
        )
    })?;
    let fields = request.fields.join(", ");
    let user = format!(
        "capability id: {id}\nprofile: predicate-i32-v1\ninput fields (fixed order): {fields}\npurpose: {purpose}\nrequest: {body}\n\nReturn only the strict JSON object.",
        id = request.capability_id,
        purpose = request.purpose,
        fields = fields,
        body = body,
    );
    Ok(GenerationPrompt {
        system: GENERATION_SYSTEM_PROMPT.to_string(),
        user,
        request_id: request.id.clone(),
        capability_id: request.capability_id.clone(),
        fields: request.fields.clone(),
    })
}

#[async_trait]
pub(crate) trait Generator: Send + Sync {
    async fn generate(
        &self,
        prompt: &GenerationPrompt,
        cancellation: &crate::RunCancellation,
    ) -> Result<String, GenerationErrorCode>;
}

/// Deterministic fake generator. It assembles `source.body` from the request contract and never
/// selects a pre-built package.
pub struct FakeGenerator {
    id: String,
    fields: Vec<String>,
    body: Mutex<FakeBody>,
    pub model_calls: AtomicUsize,
}

impl FakeGenerator {
    pub fn new(request: &RegisteredRequest, body: FakeBody) -> Self {
        Self {
            id: request.capability_id.clone(),
            fields: request.fields.clone(),
            body: Mutex::new(body),
            model_calls: AtomicUsize::new(0),
        }
    }

    pub fn set_body(&self, body: FakeBody) {
        *self.body.lock().expect("fake generator body") = body;
    }

    pub fn model_calls(&self) -> usize {
        self.model_calls.load(Ordering::SeqCst)
    }

    fn body_value(&self) -> Value {
        let first = self.fields.first().cloned().unwrap_or_default();
        let second = self.fields.get(1).cloned().unwrap_or_default();
        match *self.body.lock().expect("fake generator body") {
            FakeBody::EnabledAndNotSuspended => serde_json::json!({
                "kind": "all",
                "conditions": [
                    { "kind": "equals", "property": [first], "value": true },
                    { "kind": "equals", "property": [second], "value": false }
                ]
            }),
            FakeBody::EnabledOnly => serde_json::json!({
                "kind": "all",
                "conditions": [
                    { "kind": "equals", "property": [first], "value": true }
                ]
            }),
            FakeBody::Wrong => serde_json::json!({
                "kind": "all",
                "conditions": [
                    { "kind": "equals", "property": [second], "value": true }
                ]
            }),
        }
    }
}

#[async_trait]
impl Generator for FakeGenerator {
    async fn generate(
        &self,
        _prompt: &GenerationPrompt,
        _cancellation: &crate::RunCancellation,
    ) -> Result<String, GenerationErrorCode> {
        self.model_calls.fetch_add(1, Ordering::SeqCst);
        let fields = self
            .fields
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "kind": "boolean",
                    "values": [],
                    "nullable": false,
                    "undefinable": false,
                    "optional": false
                })
            })
            .collect::<Vec<_>>();
        Ok(serde_json::json!({
            "formatVersion": 1,
            "source": {
                "language": "l-lang",
                "version": 1,
                "id": self.id,
                "profile": "predicate-i32-v1",
                "contract": { "version": 1, "fields": fields },
                "body": self.body_value()
            }
        })
        .to_string())
    }
}

/// Provider-backed generator that fails closed when generation is not configured.
pub(crate) struct DisabledGenerator;

#[async_trait]
impl Generator for DisabledGenerator {
    async fn generate(
        &self,
        _prompt: &GenerationPrompt,
        _cancellation: &crate::RunCancellation,
    ) -> Result<String, GenerationErrorCode> {
        Err(GenerationErrorCode::Unavailable)
    }
}

#[path = "generator_providers.rs"]
mod generator_providers;
pub(crate) use generator_providers::ConversationProviderGenerator;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn request() -> RegisteredRequest {
        RegisteredRequest {
            id: "req-1".into(),
            capability_id: "enabled-user".into(),
            purpose: "Decide whether a user may proceed.".into(),
            fields: vec!["enabled".into(), "suspended".into()],
            request_path: PathBuf::from("/tmp/request.json"),
            request_hash: "a".repeat(64),
            suite_path: PathBuf::from("/tmp/tests.json"),
            suite_hash: "b".repeat(64),
            metadata_path: PathBuf::from("/tmp/metadata.json"),
            metadata_hash: "c".repeat(64),
            acceptance_id: "acc-1".into(),
            scope: super::super::config::RequestScope::User,
            allow_create: true,
            allow_update: false,
            auto_activate: true,
            grant_on_create: true,
        }
    }

    #[test]
    fn prompt_includes_the_request_but_never_the_acceptance() {
        let bytes = serde_json::json!({
            "version": 2,
            "id": "enabled-user",
            "body": "enabled が true かつ suspended が false の利用者だけを許可する。",
            "requirements": []
        })
        .to_string();
        let prompt = build_prompt(&request(), bytes.as_bytes()).unwrap();
        assert!(prompt.user.contains("enabled, suspended"));
        assert!(prompt.user.contains("suspended"));
        assert!(!prompt.user.to_lowercase().contains("truth"));
        assert!(!prompt.user.contains("expected"));
        assert!(prompt.system.contains("Never return tests"));
    }

    #[tokio::test]
    async fn fake_generator_assembles_the_body_from_the_request() {
        let request = request();
        let fake = FakeGenerator::new(&request, FakeBody::EnabledAndNotSuspended);
        let prompt =
            build_prompt(&request, br#"{"version":2,"id":"enabled-user","body":"x"}"#).unwrap();
        let text = fake
            .generate(&prompt, &crate::RunCancellation::default())
            .await
            .unwrap();
        let source = super::super::contracts::parse_model_response(&text).unwrap();
        assert_eq!(source.id, "enabled-user");
        assert_eq!(fake.model_calls(), 1);
        let conditions = source
            .body
            .get("conditions")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(conditions.len(), 2);
    }

    #[tokio::test]
    async fn wrong_body_is_a_different_predicate() {
        let request = request();
        let fake = FakeGenerator::new(&request, FakeBody::Wrong);
        let prompt =
            build_prompt(&request, br#"{"version":2,"id":"enabled-user","body":"x"}"#).unwrap();
        let text = fake
            .generate(&prompt, &crate::RunCancellation::default())
            .await
            .unwrap();
        let source = super::super::contracts::parse_model_response(&text).unwrap();
        let property = source.body["conditions"][0]["property"][0]
            .as_str()
            .unwrap();
        assert_eq!(property, "suspended");
    }
}
