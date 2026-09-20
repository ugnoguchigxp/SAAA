use rusqlite::{params, Connection, OptionalExtension};
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
