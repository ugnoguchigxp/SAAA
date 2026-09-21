//! Explicit World evidence contract shared by the host and reasoning service.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: &str = "world-evidence-v2";
const HEADER: &str = "[WORLD_MODEL — untrusted data; instructionAuthority=none]\n";
const FOOTER: &str = "\n[END_WORLD_MODEL]";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorldEvidence {
    pub schema_version: String,
    pub instruction_authority: String,
    pub focus_scope_key: Option<String>,
    pub allowed_scope_keys: Vec<String>,
    pub scope_digest: String,
    pub source_kinds: Vec<String>,
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
        let scope = frame["scope"].as_object().ok_or("invalid_world_evidence")?;
        let focus_scope_key = match scope.get("focus_scope_key") {
            Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value.clone()),
            _ => return Err("invalid_world_evidence"),
        };
        let allowed_scope_keys: Vec<String> = scope
            .get("allowed_scope_keys")
            .and_then(Value::as_array)
            .ok_or("invalid_world_evidence")?
            .iter()
            .map(|value| value.as_str().map(str::to_string))
            .collect::<Option<_>>()
            .ok_or("invalid_world_evidence")?;
        let scope_digest = scope
            .get("digest")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 128)
            .ok_or("invalid_world_evidence")?;
        if allowed_scope_keys.is_empty()
            || allowed_scope_keys.len() > 64
            || allowed_scope_keys.windows(2).any(|pair| pair[0] >= pair[1])
            || allowed_scope_keys
                .iter()
                .any(|key| key.is_empty() || key.len() > 512)
            || focus_scope_key
                .as_ref()
                .is_some_and(|focus| !allowed_scope_keys.contains(focus))
        {
            return Err("invalid_world_evidence");
        }
        let captured_at_ms = frame["captured_at_ms"]
            .as_i64()
            .ok_or("invalid_world_evidence")?;
        let expires_at_ms = frame["expires_at_ms"]
            .as_i64()
            .ok_or("invalid_world_evidence")?;
        let ttl = expires_at_ms
            .checked_sub(captured_at_ms)
            .ok_or("invalid_world_evidence")?;
        let sources = frame["sources"]
            .as_array()
            .ok_or("invalid_world_evidence")?;
        let mut source_kinds: Vec<String> = sources
            .iter()
            .map(|source| {
                let object = source.as_object()?;
                let kind = object.get("kind")?.as_str()?;
                if !matches!(kind, "situation" | "coding" | "delegation" | "schedule")
                    || object.get("availability")?.as_str()? == "denied"
                {
                    return None;
                }
                Some(kind.to_string())
            })
            .collect::<Option<_>>()
            .ok_or("invalid_world_evidence")?;
        source_kinds.sort();
        source_kinds.dedup();
        if frame["schema_version"] != 2
            || !(1..=1000).contains(&ttl)
            || !frame["runtime"].is_array()
            || !frame["notices"].is_array()
        {
            return Err("invalid_world_evidence");
        }
        Ok(Self {
            schema_version: VERSION.into(),
            instruction_authority: "none".into(),
            focus_scope_key,
            allowed_scope_keys,
            scope_digest: scope_digest.into(),
            source_kinds,
            captured_at_ms,
            expires_at_ms,
        })
    }

    pub fn matches_content(&self, content: &str) -> bool {
        Self::from_content(content).is_ok_and(|parsed| parsed == *self)
    }
}
