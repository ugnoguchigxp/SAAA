use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

pub(crate) fn backup_connection_to(source: &Connection, path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Database backup path has no parent directory".to_string())?;
    let temporary = tempfile::Builder::new()
        .prefix(".saaa-backup-")
        .suffix(".sqlite3.partial")
        .tempfile_in(parent)
        .map_err(|error| format!("Could not create the database backup: {error}"))?;
    let mut destination = Connection::open(temporary.path())
        .map_err(|error| format!("Could not open the database backup: {error}"))?;
    {
        let backup = rusqlite::backup::Backup::new(source, &mut destination)
            .map_err(|error| format!("Could not initialize the database backup: {error}"))?;
        backup
            .run_to_completion(32, Duration::from_millis(20), None)
            .map_err(|error| format!("Database backup failed: {error}"))?;
    }
    drop(destination);
    temporary
        .persist_noclobber(path)
        .map_err(|error| format!("Could not finalize the database backup: {}", error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn database_backup_is_reopenable_and_preserves_data() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source_path = directory.path().join("source.sqlite3");
        let backup_path = directory.path().join("backup.sqlite3");
        let source = Connection::open(&source_path).expect("source opens");
        crate::initialize_database(&source).expect("source initializes");
        source
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('backup-conversation', NULL, 'conversation', 'now', 'now')",
                [],
            )
            .expect("conversation inserts");
        backup_connection_to(&source, &backup_path).expect("backup succeeds");
        let backup = Connection::open(backup_path).expect("backup reopens");
        let count: i64 = backup
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id = 'backup-conversation'",
                [],
                |row| row.get(0),
            )
            .expect("backup data loads");
        assert_eq!(count, 1);
    }

    #[test]
    fn database_backup_does_not_replace_an_existing_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let backup_path = directory.path().join("backup.sqlite3");
        std::fs::write(&backup_path, b"keep this backup").expect("existing backup writes");
        let source = Connection::open_in_memory().expect("source opens");
        crate::initialize_database(&source).expect("source initializes");

        let error = backup_connection_to(&source, &backup_path)
            .expect_err("an existing backup must not be overwritten");

        assert!(error.contains("finalize"));
        assert_eq!(
            std::fs::read(&backup_path).expect("existing backup reads"),
            b"keep this backup"
        );
        let entries = std::fs::read_dir(directory.path())
            .expect("backup directory reads")
            .collect::<Result<Vec<_>, _>>()
            .expect("backup entries read");
        assert_eq!(entries.len(), 1, "temporary backup was not cleaned up");
    }
}
