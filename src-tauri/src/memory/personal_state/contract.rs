use crate::database_error;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Certification {
    pub model: String,
    pub principal: String,
    pub release: String,
    pub runtime: String,
    pub allocation: String,
    pub endpoint: String,
    pub tokenizer_digest: String,
    pub capability: String,
    pub native_tokens: u64,
    pub output_reserve: u64,
    pub safety_margin: u64,
    pub max_input_tokens: u64,
    pub max_bytes: usize,
    pub expires_at: i64,
    pub lease_epoch: u64,
    pub source_delivery_verified: bool,
    pub cleanup_verified: bool,
    pub base_snapshot_safe: bool,
    pub semantic_verified: bool,
    pub cancellation_verified: bool,
}
impl Certification {
    pub fn check(&self, principal: &str, now: i64) -> Result<(), String> {
        if self.principal != principal
            || self.expires_at <= now
            || self.release.is_empty()
            || self.runtime.is_empty()
            || self.allocation.is_empty()
            || self.tokenizer_digest.len() != 64
            || self.capability != "llm.coding"
            || !self.source_delivery_verified
            || !self.cleanup_verified
            || !self.base_snapshot_safe
            || !self.semantic_verified
            || !self.cancellation_verified
        {
            return Err("personal-contract-unverified".into());
        }
        let url = url::Url::parse(&self.endpoint).map_err(|_| "personal-contract-endpoint")?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("personal-contract-endpoint".into());
        }
        self.budget()
            .input_limit()
            .map_err(|_| "personal-contract-budget")?;
        if !(4..=268435456).contains(&self.max_bytes) {
            return Err("personal-contract-budget".into());
        }
        Ok(())
    }
    pub fn budget(&self) -> saaa_personal_state_core::Budget {
        saaa_personal_state_core::Budget {
            certified: true,
            native_tokens: self.native_tokens,
            output_reserve: self.output_reserve,
            safety_margin: self.safety_margin,
            requested_input: self.max_input_tokens,
            max_bytes: self.max_bytes as u64,
        }
    }
}
pub fn load(c: &Connection) -> Result<Certification, String> {
    let raw: String = c
        .query_row(
            "SELECT value_json FROM personal_contract WHERE id=1",
            [],
            |r| r.get(0),
        )
        .map_err(|_| "personal-contract-unconfigured")?;
    let cert: Certification = super::decode(raw)?;
    let principal: String = c
        .query_row("SELECT principal FROM personal_scope", [], |r| r.get(0))
        .map_err(database_error)?;
    cert.check(&principal, super::now())?;
    Ok(cert)
}
