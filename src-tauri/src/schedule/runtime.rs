use crate::database_error;
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Clone, Debug)]
pub(crate) struct RuntimeSettings {
    pub(crate) enabled: bool,
    pub(crate) calendar_enabled: bool,
    pub(crate) calendar_id: Option<String>,
    pub(crate) last_error: Option<String>,
    pub(crate) oauth_client_id: Option<String>,
}

pub(crate) fn load(connection: &Connection) -> Result<RuntimeSettings, String> {
    connection
        .query_row(
            "SELECT enabled, calendar_enabled, calendar_id, last_error, oauth_client_id FROM schedule_runtime WHERE id=1",
            [],
            |row| {
                Ok(RuntimeSettings {
                    enabled: row.get::<_, i64>(0)? == 1,
                    calendar_enabled: row.get::<_, i64>(1)? == 1,
                    calendar_id: row.get(2)?,
                    last_error: row.get(3)?,
                    oauth_client_id: row.get(4)?,
                })
            },
        )
        .map_err(database_error)
}

pub(crate) fn set_enabled(connection: &Connection, enabled: bool) -> Result<(), String> {
    connection
        .execute(
            "UPDATE schedule_runtime SET enabled=?1 WHERE id=1",
            [i64::from(enabled)],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn set_calendar(
    connection: &Connection,
    enabled: bool,
    calendar_id: Option<&str>,
) -> Result<(), String> {
    connection
        .execute(
            "UPDATE schedule_runtime SET calendar_enabled=?1, calendar_id=?2 WHERE id=1",
            params![i64::from(enabled), calendar_id],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn set_oauth_client(connection: &Connection, id: Option<&str>) -> Result<(), String> {
    connection
        .execute(
            "UPDATE schedule_runtime SET oauth_client_id=?1 WHERE id=1",
            [id],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn set_error(connection: &Connection, error: Option<&str>) -> Result<(), String> {
    connection
        .execute(
            "UPDATE schedule_runtime SET last_error=?1 WHERE id=1",
            [error],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn notice_once(connection: &Connection, key: &str, now: i64) -> Result<bool, String> {
    let exists: Option<String> = connection
        .query_row(
            "SELECT key FROM schedule_notices WHERE key=?1",
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    if exists.is_some() {
        return Ok(false);
    }
    connection
        .execute(
            "INSERT INTO schedule_notices(key, created_at) VALUES(?1,?2)",
            params![key, now],
        )
        .map_err(database_error)?;
    Ok(true)
}

pub(crate) fn classification_for(
    connection: &Connection,
    subject_ref: &str,
    payload_id: Option<&str>,
) -> Result<String, String> {
    if let Some(payload_id) = payload_id {
        if let Some(value) = connection
            .query_row(
                "SELECT classification FROM schedule_payloads WHERE id=?1",
                [payload_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(database_error)?
        {
            return Ok(value);
        }
    }
    if let Some(value) = connection
        .query_row(
            "SELECT json_extract(metadata,'$.classification') FROM personal_assertions WHERE id=?1",
            [subject_ref],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(database_error)?
        .flatten()
    {
        return Ok(value);
    }
    Ok("internal".into())
}
