use crate::database_error;
use rusqlite::{params, Connection};

pub(crate) fn forget(connection: &Connection, id: &str, now: i64) -> Result<(), String> {
    connection
        .execute(
            "INSERT OR IGNORE INTO schedule_tombstones(id, forgotten_at) VALUES(?1,?2)",
            params![id, now],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn recover(connection: &Connection, known: &[(String, i64)]) -> Result<(), String> {
    for (id, at) in known {
        if !(id.starts_with("schp_") || id.starts_with("scho_")) {
            continue;
        }
        forget(connection, id, *at)?;
    }
    Ok(())
}

pub(crate) fn forgotten(connection: &Connection, id: &str) -> Result<bool, String> {
    let count: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM schedule_tombstones WHERE id=?1)",
            [id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    Ok(count == 1)
}
