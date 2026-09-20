use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Stores the currently configured role-routing policy once. Re-saving identical JSON is a no-op;
/// roots therefore always reference the exact policy they began with.
pub(crate) fn capture_current_policy(connection: &Connection, now_ms: i64) -> Result<(), String> {
    let config_json: Option<String> = connection
        .query_row(
            "SELECT value_json FROM settings_documents WHERE namespace='routing.roles' AND key='default'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(config_json) = config_json else {
        return Ok(());
    };
    // Settings documents originate in JSON, whose object key order is not meaningful. Persist a
    // canonical snapshot so a UI re-save that only rearranges fields cannot create a new policy
    // version for subsequent roots.
    let config_json = canonicalize_json(&config_json)?;
    let digest = format!("{:x}", Sha256::digest(config_json.as_bytes()));
    let previous: Option<String> = connection
        .query_row(
            "SELECT digest FROM rr_policy_versions ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if previous.as_deref() == Some(digest.as_str()) {
        return Ok(());
    }
    let version: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(version), 0) + 1 FROM rr_policy_versions",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO rr_policy_versions(id, version, config_json, digest, created_at_ms) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![format!("rr-policy-{version}"), version, config_json, digest, now_ms],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn canonicalize_json(input: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(input)
        .map_err(|error| format!("Could not decode role-routing policy: {error}"))?;
    serde_json::to_string(&canonicalize_value(value))
        .map_err(|error| format!("Could not encode role-routing policy: {error}"))
}

fn canonicalize_value(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize_value).collect()),
        Value::Object(entries) => {
            let mut entries = entries.into_iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize_value(value)))
                    .collect(),
            )
        }
        other => other,
    }
}
