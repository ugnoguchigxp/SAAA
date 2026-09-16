//! Pinned Context API (schema 1). Provisioning is a separate host contract:
//! registration never accepts raw text. Credentials and body errors are not logged.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Registration {
    pub id: String,
    pub version: String,
    pub source_handle: String,
    pub source_digest: String,
    pub classification: String,
    pub byte_count: u64,
    pub token_count: u64,
    pub tokenizer_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanItem {
    pub context_id: String,
    pub version: String,
    pub required: bool,
    pub utility: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewRequest {
    pub allocation_id: String,
    pub runtime: String,
    pub base_input_tokens: u64,
    pub max_input_tokens: u64,
    pub deadline: String,
    pub canonicalization_version: String,
    pub items: Vec<PlanItem>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewItem {
    pub context_id: String,
    pub version: String,
    pub required: bool,
    pub utility: f64,
    pub token_count: u64,
    pub source_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Omission {
    pub context_id: String,
    pub version: String,
    pub reason: String,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct View {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_epoch: Option<u64>,
    pub schema_version: u32,
    pub id: String,
    pub operation_id: String,
    // Public API removes the principal; identity comes from the credential binding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    pub allocation_id: String,
    pub runtime: String,
    pub release: String,
    pub compatibility_key: String,
    pub view_digest: String,
    pub canonicalization_version: String,
    pub base_input_tokens: u64,
    pub input_budget_tokens: u64,
    pub token_count: u64,
    pub ordered_items: Vec<ViewItem>,
    pub omitted: Vec<Omission>,
    pub lease_epoch: u64,
    pub state: String,
    pub created_at: String,
    pub expires_at: String,
}
impl View {
    pub fn validate(
        &self,
        request: &ViewRequest,
        release: &str,
        lease_epoch: u64,
        now_ms: i64,
    ) -> Result<(), String> {
        let expires = chrono::DateTime::parse_from_rfc3339(&self.expires_at)
            .map_err(|_| "context-view-expiry")?
            .timestamp_millis();
        if self.schema_version != 1
            || self.state != "ready"
            || self.allocation_id != request.allocation_id
            || self.runtime != request.runtime
            || self.release != release
            || self.lease_epoch != lease_epoch
            || expires <= now_ms
            || self.canonicalization_version != "context-view-v1"
            || self.base_input_tokens != request.base_input_tokens
            || self.input_budget_tokens > request.max_input_tokens
            || self.token_count > self.input_budget_tokens
        {
            return Err("context-view-binding".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for item in &self.ordered_items {
            if !seen.insert((&item.context_id, &item.version))
                || !request.items.iter().any(|r| {
                    r.context_id == item.context_id
                        && r.version == item.version
                        && r.required == item.required
                })
            {
                return Err("context-view-items".into());
            }
        }
        for omission in &self.omitted {
            if !seen.insert((&omission.context_id, &omission.version))
                || !matches!(
                    omission.reason.as_str(),
                    "budget" | "not_found" | "invalid" | "tokenizer_mismatch"
                )
                || !request.items.iter().any(|r| {
                    r.context_id == omission.context_id
                        && r.version == omission.version
                        && !r.required
                })
            {
                return Err("context-view-required-missing".into());
            }
        }
        if seen.len() != request.items.len() {
            return Err("context-view-items".into());
        }
        Ok(())
    }
}

pub struct Client {
    http: reqwest::Client,
    base: url::Url,
    token: zeroize::Zeroizing<String>,
}
impl Client {
    pub fn new(endpoint: &str, token: String) -> Result<Self, String> {
        let base = url::Url::parse(endpoint).map_err(|_| "context-endpoint")?;
        if !matches!(base.scheme(), "http" | "https")
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            return Err("context-endpoint".into());
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "context-client")?,
            base,
            token: zeroize::Zeroizing::new(token),
        })
    }
    pub(super) fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, String> {
        Ok(self
            .http
            .request(
                method,
                self.base.join(path).map_err(|_| "context-endpoint")?,
            )
            .bearer_auth(self.token.as_str()))
    }
    pub(super) async fn body(request: reqwest::RequestBuilder) -> Result<Value, String> {
        let mut response = request.send().await.map_err(|_| "context-transport")?;
        if !response.status().is_success() {
            return Err(format!("context-http-{}", response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        while let Some(part) = response.chunk().await.map_err(|_| "context-transport")? {
            if bytes.len() + part.len() > 1024 * 1024 {
                return Err("context-response-limit".into());
            }
            bytes.extend_from_slice(&part);
        }
        serde_json::from_slice(&bytes).map_err(|_| "context-response-schema".into())
    }
    pub async fn register(&self, registration: &Registration, key: &str) -> Result<Value, String> {
        if registration.byte_count == 0
            || registration.token_count == 0
            || registration.token_count > 20_000_000
            || registration.source_digest.len() != 64
            || registration.tokenizer_digest.len() != 64
            || !matches!(
                registration.classification.as_str(),
                "public" | "internal" | "confidential" | "restricted"
            )
        {
            return Err("context-registration-invalid".into());
        }
        Self::body(
            self.request(reqwest::Method::POST, "/v1/contexts")?
                .header("idempotency-key", key)
                .json(registration),
        )
        .await
    }
    pub async fn create_view(&self, request: &ViewRequest, key: &str) -> Result<View, String> {
        let mut seen = std::collections::BTreeSet::new();
        if request.items.is_empty()
            || request.items.len() > 512
            || request.canonicalization_version != "context-view-v1"
            || request.items.iter().any(|i| {
                !seen.insert((&i.context_id, &i.version))
                    || !i.utility.is_finite()
                    || !(0.0..=1.0).contains(&i.utility)
            })
        {
            return Err("context-view-invalid".into());
        }
        let value = Self::body(
            self.request(reqwest::Method::POST, "/v1/context-views")?
                .header("idempotency-key", key)
                .json(request),
        )
        .await?;
        serde_json::from_value(value).map_err(|_| "context-view-schema".into())
    }
    pub async fn operation(&self, id: &str) -> Result<Value, String> {
        validate_id(id)?;
        Self::body(self.request(
            reqwest::Method::GET,
            &format!("/v1/context-operations/{id}"),
        )?)
        .await
    }
    /// Success means only HTTP acknowledgement, never physical deletion.
    pub async fn delete_registration(&self, id: &str, key: &str) -> Result<(), String> {
        validate_id(id)?;
        let response = self
            .request(reqwest::Method::DELETE, &format!("/v1/contexts/{id}"))?
            .header("idempotency-key", key)
            .send()
            .await
            .map_err(|_| "context-transport")?;
        if response.status() != reqwest::StatusCode::NO_CONTENT {
            return Err("context-delete-unconfirmed".into());
        }
        Ok(())
    }
    pub async fn registration_absent(&self, id: &str) -> Result<bool, String> {
        let mut cursor: Option<String> = None;
        let mut cursors = std::collections::BTreeSet::new();
        for _ in 0..1000 {
            let mut request = self
                .request(reqwest::Method::GET, "/v1/contexts")?
                .query(&[("limit", "500")]);
            if let Some(c) = &cursor {
                request = request.query(&[("cursor", c)]);
            }
            let value = Self::body(request).await?;
            let contexts = value
                .get("contexts")
                .and_then(Value::as_array)
                .ok_or("context-list-schema")?;
            if contexts
                .iter()
                .any(|c| c.get("id").and_then(Value::as_str) == Some(id))
            {
                return Ok(false);
            }
            cursor = value
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            match &cursor {
                None => return Ok(true),
                Some(c) if !cursors.insert(c.clone()) => return Err("context-list-loop".into()),
                _ => {}
            }
        }
        Err("context-list-incomplete".into())
    }
    /// Authorized base-only inference is explicitly distinct from KV:mem.
    pub async fn base_chat(
        &self,
        allocation: &str,
        capability: &str,
        request: &Value,
    ) -> Result<Value, String> {
        if allocation.is_empty() || capability != "llm.coding" {
            return Err("context-chat-binding".into());
        }
        Self::body(
            self.request(reqwest::Method::POST, "/v1/chat/completions")?
                .header("x-larm-allocation-id", allocation)
                .header("x-larm-capability", capability)
                .json(request),
        )
        .await
    }
    /// The caller must atomically consume its persisted manifest BEFORE invoking.
    pub async fn chat(
        &self,
        view: &View,
        capability: &str,
        request: &Value,
    ) -> Result<Value, String> {
        if view.state != "ready" || capability != "llm.coding" {
            return Err("context-chat-binding".into());
        }
        Self::body(
            self.request(reqwest::Method::POST, "/v1/chat/completions")?
                .header("x-larm-allocation-id", &view.allocation_id)
                .header("x-larm-context-view-id", &view.id)
                .header("x-larm-capability", capability)
                .json(request),
        )
        .await
    }
}
fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 192
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        Err("context-id".into())
    } else {
        Ok(())
    }
}
