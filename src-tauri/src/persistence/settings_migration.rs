use rusqlite::{params, Connection};

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

pub(crate) fn migrate_settings_v9_to_v10(connection: &Connection) -> rusqlite::Result<()> {
    add_default(
        connection,
        "providers.model",
        "default",
        "maxOutputTokens",
        crate::providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
    )?;
    add_default(
        connection,
        "voice.runtime",
        "default",
        "sttHost",
        crate::voice::network_asr::DEFAULT_HOST,
    )?;
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
