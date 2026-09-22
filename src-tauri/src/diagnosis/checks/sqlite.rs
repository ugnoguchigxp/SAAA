use super::item;
use crate::diagnosis::contract::{DiagnosisItem, DiagnosisSeverity, DiagnosisStatus};
use crate::persistence::schema::DATABASE_SCHEMA_VERSION;
use crate::AppState;

pub(in crate::diagnosis) fn sqlite(state: &AppState) -> DiagnosisItem {
    match state.sqlite_readers.read(|connection| {
        connection
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .map_err(crate::database_error)?;
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(crate::database_error)
    }) {
        Ok(version) if version == DATABASE_SCHEMA_VERSION => item(
            "sqlite",
            "storage",
            "SQLite",
            DiagnosisStatus::Ok,
            DiagnosisSeverity::Fatal,
            "",
            None,
        ),
        Ok(version) => item(
            "sqlite",
            "storage",
            "SQLite",
            DiagnosisStatus::Warn,
            DiagnosisSeverity::Fatal,
            &format!("schema version is {version}, expected {DATABASE_SCHEMA_VERSION}"),
            None,
        ),
        Err(error) => item(
            "sqlite",
            "storage",
            "SQLite",
            DiagnosisStatus::Fail,
            DiagnosisSeverity::Fatal,
            &error,
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> AppState {
        let connection = Connection::open_in_memory().expect("database opens");
        crate::initialize_database(&connection).expect("database initializes");
        crate::test_support::app_state(connection)
    }

    #[test]
    fn dg_03_sqlite_ok_on_initialized_database() {
        let item = sqlite(&fresh());
        assert_eq!(item.status, DiagnosisStatus::Ok);
        assert_eq!(item.severity, DiagnosisSeverity::Fatal);
        assert_eq!(item.id, "sqlite");
    }

    #[test]
    fn dg_03_sqlite_warns_on_version_mismatch() {
        let state = fresh();
        state
            .sqlite_writer
            .write(|connection| {
                connection
                    .pragma_update(None, "user_version", 1i64)
                    .map_err(crate::database_error)?;
                Ok(())
            })
            .expect("version changes");
        let item = sqlite(&state);
        assert_eq!(item.status, DiagnosisStatus::Warn);
        assert_eq!(item.severity, DiagnosisSeverity::Fatal);
    }
}
