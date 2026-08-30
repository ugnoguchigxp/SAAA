use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::settings::SETTINGS_SCHEMA_VERSION;
use crate::{now_iso, DEFAULT_AGENT_NAME, DEFAULT_USER_NAME};

fn add_default(
    connection: &Connection,
    namespace: &str,
    key: &str,
    field: &str,
    value: impl rusqlite::ToSql,
) -> rusqlite::Result<()> {
    connection.execute(
        &format!(
            "UPDATE settings_documents
             SET value_json=json_set(value_json, '$.{field}', ?1), updated_at=?2
             WHERE namespace=?3 AND key=?4 AND json_valid(value_json)
               AND json_type(value_json, '$.{field}') IS NULL"
        ),
        params![value, now_iso(), namespace, key],
    )?;
    Ok(())
}

fn migration_asr_host(connection: &Connection) -> rusqlite::Result<String> {
    let providers: Option<String> = connection
        .query_row(
            "SELECT value_json FROM settings_documents
             WHERE namespace='providers.model' AND key='default' AND json_valid(value_json)",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let discovered = providers
        .as_deref()
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .and_then(|value| value.get("providers")?.as_array().cloned())
        .and_then(|providers| {
            providers.into_iter().find_map(|provider| {
                let host = provider.get("host")?.as_str()?.trim();
                (provider.get("kind")?.as_str()? == "dynamic-lan"
                    && crate::voice::network_asr::base_url_from_host(host).is_ok())
                .then(|| host.to_string())
            })
        });
    Ok(discovered.unwrap_or_else(|| crate::voice::network_asr::DEFAULT_HOST.to_string()))
}

pub(crate) fn migrate_settings_to_current(connection: &Connection) -> rusqlite::Result<()> {
    add_default(
        connection,
        "providers.model",
        "default",
        "maxOutputTokens",
        crate::providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
    )?;
    connection.execute(
        "UPDATE settings_documents
         SET value_json=json_remove(value_json, '$.outputDeviceId'), updated_at=?1
         WHERE namespace='voice.runtime' AND key='default'
           AND json_valid(value_json) AND json_type(value_json, '$.outputDeviceId') IS NOT NULL",
        [now_iso()],
    )?;
    let asr_host = migration_asr_host(connection)?;
    add_default(connection, "voice.runtime", "default", "sttHost", asr_host)?;
    add_default(
        connection,
        "providers.agent",
        "codex-sdk",
        "agentName",
        DEFAULT_AGENT_NAME,
    )?;
    add_default(
        connection,
        "providers.agent",
        "codex-sdk",
        "userName",
        DEFAULT_USER_NAME,
    )?;
    connection.execute(
        "UPDATE settings_documents SET schema_version=?1, updated_at=?2 WHERE schema_version < ?1",
        params![SETTINGS_SCHEMA_VERSION, now_iso()],
    )?;
    Ok(())
}
