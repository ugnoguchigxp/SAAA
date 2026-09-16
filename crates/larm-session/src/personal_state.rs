//! Authenticated product contract. Callers persist operation IDs before network IO.
use crate::contexts::{Client, View};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const VERSION: &str = "larm-personal-state.v1";
/// Production limit certified for the Qwen 3.8 release used by SAAA.
pub const MAX_CERTIFIED_INPUT_TOKENS: u64 = 125_000;
/// Output budget reserved by the certified 128k context configuration.
pub const MAX_CERTIFIED_OUTPUT_TOKENS: u64 = 4_096;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capability {
    pub contract_version: String,
    pub boot_epoch: String,
    pub subject_digest: String,
    pub allocation_id: String,
    pub runtime: String,
    pub release: String,
    pub lease_epoch: u64,
    pub lease_expires_at: String,
    pub credential_expires_at: String,
    pub tokenizer_digest: String,
    pub chat_template_digest: String,
    pub context_limit_tokens: u64,
    pub output_reserve_tokens: u64,
    pub safety_margin_tokens: u64,
    pub source_token_limit: u64,
    pub max_source_bytes: u64,
    pub max_total_source_bytes: u64,
    pub max_materialized_bytes: u64,
    pub scopes: Vec<String>,
}
pub fn instant(s: &str) -> Result<i64, String> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.timestamp_millis())
        .map_err(|_| "personal-receipt-expiry".into())
}
pub fn identifier(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    {
        return Err("personal-operation-id".into());
    }
    Ok(())
}
impl Capability {
    pub fn expires_at(&self) -> Result<i64, String> {
        Ok(instant(&self.lease_expires_at)?.min(instant(&self.credential_expires_at)?))
    }
    pub fn input_limit(&self) -> Result<u64, String> {
        Ok(self
            .context_limit_tokens
            .checked_sub(self.output_reserve_tokens)
            .and_then(|n| n.checked_sub(self.safety_margin_tokens))
            .filter(|n| *n > 0)
            .ok_or("personal-capability-budget")?
            .min(MAX_CERTIFIED_INPUT_TOKENS))
    }
    pub fn output_limit(&self) -> Result<u64, String> {
        let limit = self
            .output_reserve_tokens
            .min(MAX_CERTIFIED_OUTPUT_TOKENS);
        (limit > 0)
            .then_some(limit)
            .ok_or("personal-capability-budget".into())
    }
    pub fn validate(
        &self,
        subject: &str,
        allocation: &str,
        runtime: &str,
        now: i64,
    ) -> Result<(), String> {
        let required = [
            "context.source.provision",
            "context.measure",
            "context.view.create",
            "context.generate",
            "context.attempt.cancel",
            "context.forget",
            "context.operation.read",
        ];
        if self.contract_version != VERSION
            || self.subject_digest != subject
            || self.allocation_id != allocation
            || self.runtime != runtime
            || self.release.is_empty()
            || self.expires_at()? <= now
            || uuid::Uuid::parse_str(&self.boot_epoch).is_err()
            || self.scopes.len() != required.len()
            || required.iter().any(|s| !self.scopes.iter().any(|v| v == s))
            || [
                &self.subject_digest,
                &self.tokenizer_digest,
                &self.chat_template_digest,
            ]
            .iter()
            .any(|s| {
                s.len() != 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
            || self.max_materialized_bytes == 0
            || self.max_source_bytes == 0
            || self.source_token_limit == 0
        {
            return Err("personal-capability-binding".into());
        }
        self.input_limit()?;
        self.output_limit()?;
        Ok(())
    }
    pub fn receipt(&self, v: &Value) -> Result<(), String> {
        if v["contractVersion"] != VERSION
            || v["subjectDigest"] != self.subject_digest
            || v["allocationId"] != self.allocation_id
            || v["runtime"] != self.runtime
            || v["release"] != self.release
            || v["leaseEpoch"] != self.lease_epoch
            || v["tokenizerDigest"] != self.tokenizer_digest
            || v["chatTemplateDigest"] != self.chat_template_digest
            || v["dataEpoch"].as_u64().is_none()
            || instant(v["expiresAt"].as_str().ok_or("personal-receipt-expiry")?)?
                <= chrono::Utc::now().timestamp_millis()
        {
            return Err("personal-receipt-binding".into());
        }
        Ok(())
    }
}
impl Client {
    pub async fn personal_capability(
        &self,
        allocation: &str,
        runtime: &str,
    ) -> Result<Capability, String> {
        let v = Self::body(
            self.request(reqwest::Method::GET, "/v1/personal-state/capability")?
                .header("x-larm-allocation-id", allocation)
                .header("x-larm-runtime", runtime),
        )
        .await?;
        serde_json::from_value(v).map_err(|_| "personal-capability-schema".into())
    }
    pub async fn product_get(&self, kind: &str, id: &str) -> Result<Value, String> {
        identifier(id)?;
        let path = match kind {
            "source" => "/v1/context-source-operations",
            "measurement" => "/v1/context-measurements",
            "view" => "/v2/context-views",
            "attempt" => "/v1/generation-attempts",
            "forget" => "/v1/context-forget-operations",
            _ => return Err("personal-operation-kind".into()),
        };
        Self::body(self.request(reqwest::Method::GET, &format!("{path}/{id}"))?).await
    }
    pub async fn provision_source(
        &self,
        c: &Capability,
        id: &str,
        digest: &str,
        text: &str,
    ) -> Result<Value, String> {
        identifier(id)?;
        if text.len() as u64 > c.max_source_bytes {
            return Err("personal-source-byte-limit".into());
        }
        Self::body(
            self.request(reqwest::Method::POST, "/v1/context-sources")?
                .header("content-type", "text/plain; charset=utf-8")
                .header("x-larm-source-incarnation", id)
                .header("x-larm-allocation-id", &c.allocation_id)
                .header("x-larm-runtime", &c.runtime)
                .header("x-larm-source-digest", digest)
                .body(text.to_owned()),
        )
        .await
    }
    pub async fn measure_request(
        &self,
        c: &Capability,
        id: &str,
        request: &Value,
    ) -> Result<Value, String> {
        identifier(id)?;
        Self::body(self.request(reqwest::Method::POST,"/v1/context-measurements")?.json(&json!({"measurementId":id,"allocationId":c.allocation_id,"runtime":c.runtime,"maxInputTokens":c.input_limit()?,"request":request}))).await
    }
    pub async fn product_view(&self, request: &Value, id: &str) -> Result<View, String> {
        identifier(id)?;
        let v = Self::body(
            self.request(reqwest::Method::POST, "/v2/context-views")?
                .header("idempotency-key", id)
                .json(request),
        )
        .await?;
        serde_json::from_value(v).map_err(|_| "personal-view-schema".into())
    }
    pub async fn product_chat(
        &self,
        c: &Capability,
        id: &str,
        view: Option<&str>,
        request: &Value,
    ) -> Result<Value, String> {
        identifier(id)?;
        let mut r = self
            .request(reqwest::Method::POST, "/v1/chat/completions")?
            .header("x-larm-allocation-id", &c.allocation_id)
            .header("x-larm-attempt-id", id)
            .json(request);
        if let Some(view) = view {
            r = r.header("x-larm-context-view-id", view);
        }
        Self::body(r).await
    }
    pub async fn cancel_attempt(&self, id: &str) -> Result<Value, String> {
        identifier(id)?;
        Self::body(self.request(
            reqwest::Method::POST,
            &format!("/v1/generation-attempts/{id}/cancel"),
        )?)
        .await
    }
    pub async fn product_forget(&self, request: &Value) -> Result<Value, String> {
        Self::body(
            self.request(reqwest::Method::POST, "/v1/context-forget-operations")?
                .json(request),
        )
        .await
    }
}
pub fn forget_complete(v: &Value, subject: &str, id: &str) -> bool {
    let phases = [
        "attempts",
        "views",
        "runtime",
        "snapshots",
        "registry",
        "sources",
        "audit",
    ];
    v["contractVersion"] == VERSION
        && v["subjectDigest"] == subject
        && v["forgetId"] == id
        && v["state"] == "succeeded"
        && v["absenceVerified"] == true
        && v["phases"]
            .as_object()
            .is_some_and(|p| p.len() == phases.len())
        && phases.iter().all(|p| v["phases"][p]["state"] == "absent")
}
