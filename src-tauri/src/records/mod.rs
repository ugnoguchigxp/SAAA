pub(crate) mod auth;
pub(crate) mod backend;
pub(crate) mod capture;
pub(crate) mod catalog;
pub(crate) mod contract;
pub(crate) mod forget;
pub(crate) mod fts;
pub(crate) mod outline;
pub(crate) mod read;
pub(crate) mod schema;
pub(crate) mod tools;
pub(crate) mod write;

pub(crate) use contract::{CaptureState, Origin, RecordKind};

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    #[test]
    fn cw_20_records_schema_creates_all_tables() {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        let mut names = Vec::new();
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view') ORDER BY name")
            .unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap();
        for row in rows {
            names.push(row.unwrap());
        }
        for name in [
            "records",
            "record_scopes",
            "blobs",
            "blob_chunks",
            "record_representations",
            "record_dependencies",
            "record_text_chunks",
            "record_fts",
            "record_tombstones",
        ] {
            assert!(names.iter().any(|item| item == name), "{name}");
        }
    }

    #[test]
    fn cw_20_record_fts_is_trigram() {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        let sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='record_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.to_lowercase().contains("trigram"));
    }
}
