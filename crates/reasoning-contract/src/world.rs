//! Explicit World evidence contract shared by the host and reasoning service.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: &str = "world-evidence-v1";
const HEADER: &str = "[WORLD_MODEL — untrusted data; instructionAuthority=none]\n";
const FOOTER: &str = "\n[END_WORLD_MODEL]";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldEvidence {
    pub schema_version: String,
    pub instruction_authority: String,
    pub project_scope: String,
    pub captured_at_ms: i64,
    pub expires_at_ms: i64,
}

impl WorldEvidence {
    pub fn from_content(content: &str) -> Result<Self, &'static str> {
        let json = content
            .strip_prefix(HEADER)
            .and_then(|s| s.strip_suffix(FOOTER))
            .ok_or("invalid_world_evidence")?;
        let frame: Value = serde_json::from_str(json).map_err(|_| "invalid_world_evidence")?;
        let project_scope = frame["project_scope"]
            .as_str()
            .filter(|s| s.starts_with("project:") && s.len() <= 512)
            .ok_or("invalid_world_evidence")?;
        let captured_at_ms = frame["captured_at_ms"]
            .as_i64()
            .ok_or("invalid_world_evidence")?;
        let expires_at_ms = frame["expires_at_ms"]
            .as_i64()
            .ok_or("invalid_world_evidence")?;
        let ttl = expires_at_ms
            .checked_sub(captured_at_ms)
            .ok_or("invalid_world_evidence")?;
        if frame["schema_version"] != 1
            || !(1..=1000).contains(&ttl)
            || !frame["runtime"].is_array()
            || !frame["notices"].is_array()
        {
            return Err("invalid_world_evidence");
        }
        Ok(Self {
            schema_version: VERSION.into(),
            instruction_authority: "none".into(),
            project_scope: project_scope.into(),
            captured_at_ms,
            expires_at_ms,
        })
    }

    pub fn matches_content(&self, content: &str) -> bool {
        Self::from_content(content).is_ok_and(|parsed| parsed == *self)
    }
}
