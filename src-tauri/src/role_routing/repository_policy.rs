use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Stores the currently configured role-routing policy once. Re-saving identical JSON is a no-op;
/// roots therefore always reference the exact policy they began with.
pub(crate) fn capture_current_policy(connection: &Connection, now_ms: i64) -> Result<(), String> {
    capture_policy_version(connection, None, now_ms).map(|_| ())
}

/// Captures the current role-routing settings as an immutable policy snapshot. When
/// `expected_version` is supplied the capture is a compare-and-swap: a stale caller (its expected
/// version no longer matches the latest snapshot) is rejected instead of silently creating a new
/// version over a concurrent save. Re-saving the same digest is a no-op regardless of CAS.
pub(crate) fn capture_policy_version(
    connection: &Connection,
    expected_version: Option<i64>,
    now_ms: i64,
) -> Result<i64, String> {
    let config_json: Option<String> = connection
        .query_row(
            "SELECT value_json FROM settings_documents WHERE namespace='routing.roles' AND key='default'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let Some(config_json) = config_json else {
        return Ok(0);
    };
    // Settings documents originate in JSON, whose object key order is not meaningful. Persist a
    // canonical snapshot so a UI re-save that only rearranges fields cannot create a new policy
    // version for subsequent roots.
    let config_json = canonicalize_json(&config_json)?;
    let digest = format!("{:x}", Sha256::digest(config_json.as_bytes()));
    let current_version: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM rr_policy_versions",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    if let Some(expected) = expected_version {
        if expected != current_version {
            return Err("Role-routing policy version conflict".into());
        }
    }
    let previous: Option<String> = connection
        .query_row(
            "SELECT digest FROM rr_policy_versions ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if previous.as_deref() == Some(digest.as_str()) {
        return Ok(current_version);
    }
    let version = current_version + 1;
    connection
        .execute(
            "INSERT INTO rr_policy_versions(id, version, config_json, digest, created_at_ms) VALUES(?1, ?2, ?3, ?4, ?5)",
            params![format!("rr-policy-{version}"), version, config_json, digest, now_ms],
        )
        .map_err(|error| error.to_string())?;
    Ok(version)
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fixture() -> Connection {
        let connection = Connection::open_in_memory().expect("database");
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE conversations(id TEXT PRIMARY KEY);
                 CREATE TABLE runtime_runs(id TEXT PRIMARY KEY);
                 CREATE TABLE conversation_messages(id TEXT PRIMARY KEY);
                 CREATE TABLE settings_documents(namespace TEXT, key TEXT, value_json TEXT, UNIQUE(namespace,key));",
            )
            .expect("base");
        crate::role_routing::schema::migrate(&connection).expect("schema");
        connection
    }

    fn set_policy(connection: &Connection, value: &str) {
        connection
            .execute(
                "INSERT INTO settings_documents(namespace,key,value_json) VALUES('routing.roles','default',?1)
                 ON CONFLICT(namespace,key) DO UPDATE SET value_json=excluded.value_json",
                [value],
            )
            .expect("settings document");
    }

    #[test]
    fn rr_03_policy_cas_conflict() {
        let connection = fixture();
        set_policy(&connection, "{\"enabled\":false,\"version\":1}");
        assert_eq!(
            capture_policy_version(&connection, Some(0), 1).expect("first capture"),
            1
        );
        // A stale writer that still expects version 0 must be rejected, not create version 2.
        set_policy(&connection, "{\"enabled\":false,\"version\":2}");
        assert!(capture_policy_version(&connection, Some(0), 2).is_err());
        assert_eq!(
            capture_policy_version(&connection, Some(1), 3).expect("in-order capture"),
            2
        );
        // Re-saving an identical digest is a no-op even with the current expected version.
        assert_eq!(
            capture_policy_version(&connection, Some(2), 4).expect("same digest"),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM rr_policy_versions", [], |row| row
                    .get::<_, i64>(0))
                .expect("versions"),
            2
        );
    }
}
