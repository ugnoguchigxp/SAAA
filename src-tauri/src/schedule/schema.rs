use rusqlite::Connection;

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(include_str!("schema.sql"))?;
    let _ = connection.execute(
        "ALTER TABLE schedule_runtime ADD COLUMN oauth_client_id TEXT",
        [],
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        Connection::open_in_memory().expect("db")
    }

    #[test]
    fn sl_02_migration_succeeds_on_empty_and_existing_db() {
        let blank = memory();
        migrate(&blank).expect("empty");
        migrate(&blank).expect("idempotent");
        let existing = memory();
        existing
            .execute_batch("CREATE TABLE keep(id INTEGER PRIMARY KEY); INSERT INTO keep VALUES(1);")
            .unwrap();
        migrate(&existing).expect("existing");
        let tables: i64 = existing
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN
                 ('schedule_entries','calendar_projections','calendar_observations','calendar_sync_state')",
                [],
                |row: &rusqlite::Row<'_>| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 4);
        let enabled: i64 = existing
            .query_row(
                "SELECT enabled FROM schedule_runtime WHERE id=1",
                [],
                |row: &rusqlite::Row<'_>| row.get(0),
            )
            .unwrap();
        assert_eq!(enabled, 0);
    }

    #[test]
    fn sl_02_initialize_database_adds_tables_and_bumps_user_version() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, crate::persistence::schema::DATABASE_SCHEMA_VERSION);
        let tables: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN
                 ('schedule_entries','calendar_projections','calendar_observations','calendar_sync_state')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 4);
        crate::persistence::schema::initialize_database(&connection).unwrap();
        let again: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(again, version);
    }

    #[test]
    fn sl_02_backup_before_migration_runs_when_user_version_lags() {
        let directory = tempfile::tempdir().expect("tmp");
        let path = directory.path().join("lag.sqlite3");
        let connection = rusqlite::Connection::open(&path).unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        connection.pragma_update(None, "user_version", 29).unwrap();
        let backup = crate::persistence::migrate::backup_before_migration(&connection, &path)
            .expect("backup")
            .expect("created");
        assert!(backup.exists());
        crate::persistence::schema::initialize_database(&connection).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, crate::persistence::schema::DATABASE_SCHEMA_VERSION);
    }
}
