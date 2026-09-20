use rusqlite::Connection;

pub(crate) fn migrate(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(include_str!("schema.sql"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> Connection {
        Connection::open_in_memory().expect("db")
    }

    #[test]
    fn sl_02_migration_succeeds_on_empty_and_existing_db() {
        let empty = empty();
        migrate(&empty).expect("empty");
        migrate(&empty).expect("idempotent");
        let existing = empty();
        existing
            .execute_batch("CREATE TABLE keep(id INTEGER PRIMARY KEY); INSERT INTO keep VALUES(1);")
            .unwrap();
        migrate(&existing).expect("existing");
        let tables: i64 = existing
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN
                 ('schedule_entries','calendar_projections','calendar_observations','calendar_sync_state')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 4);
        let enabled: i64 = existing
            .query_row("SELECT enabled FROM schedule_runtime WHERE id=1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(enabled, 0);
    }
}
