#[cfg(any(test, feature = "quality-eval-harness"))]
use super::SqliteWriter;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex, TryLockError,
};

const READER_LANES: usize = 2;

#[derive(Clone)]
pub(crate) struct SqliteReaders {
    source: ReaderSource,
}

#[derive(Clone)]
enum ReaderSource {
    Persistent(Arc<PersistentReaders>),
    #[cfg(any(test, feature = "quality-eval-harness"))]
    Serialized(Arc<SqliteWriter>),
}

struct PersistentReaders {
    lanes: [Mutex<Connection>; READER_LANES],
    next_lane: AtomicUsize,
    settings_snapshot: Mutex<Option<CachedSettingsSnapshot>>,
}

struct CachedSettingsSnapshot {
    revision: i64,
    documents: Vec<crate::SettingsDocument>,
}

impl SqliteReaders {
    pub(crate) fn open(database_path: &Path) -> Result<Self, String> {
        let first = open_reader(database_path)?;
        let second = open_reader(database_path)?;
        Ok(Self {
            source: ReaderSource::Persistent(Arc::new(PersistentReaders {
                lanes: [Mutex::new(first), Mutex::new(second)],
                next_lane: AtomicUsize::new(0),
                settings_snapshot: Mutex::new(None),
            })),
        })
    }

    #[cfg(any(test, feature = "quality-eval-harness"))]
    pub(crate) fn serialized(writer: Arc<SqliteWriter>) -> Self {
        Self {
            source: ReaderSource::Serialized(writer),
        }
    }

    pub(crate) fn read<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        match &self.source {
            ReaderSource::Persistent(readers) => {
                let lane = readers.next_lane.fetch_add(1, Ordering::Relaxed) % READER_LANES;
                let alternate = (lane + 1) % READER_LANES;
                let mut connection = match readers.lanes[lane].try_lock() {
                    Ok(connection) => connection,
                    Err(TryLockError::WouldBlock) => match readers.lanes[alternate].try_lock() {
                        Ok(connection) => connection,
                        Err(TryLockError::WouldBlock) => readers.lanes[lane]
                            .lock()
                            .map_err(|_| "Database reader unavailable".to_string())?,
                        Err(TryLockError::Poisoned(_)) => {
                            return Err("Database reader unavailable".to_string())
                        }
                    },
                    Err(TryLockError::Poisoned(_)) => {
                        return Err("Database reader unavailable".to_string())
                    }
                };
                let transaction = connection.transaction().map_err(crate::database_error)?;
                let result = operation(&transaction)?;
                transaction.commit().map_err(crate::database_error)?;
                Ok(result)
            }
            #[cfg(any(test, feature = "quality-eval-harness"))]
            ReaderSource::Serialized(writer) => writer.read_serialized(operation),
        }
    }

    pub(crate) async fn read_async<T, F>(&self, operation: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, String> + Send + 'static,
    {
        let readers = self.clone();
        tauri::async_runtime::spawn_blocking(move || readers.read(operation))
            .await
            .map_err(|error| format!("Database reader task failed: {error}"))?
    }

    /// Returns the validated seven-document settings snapshot without decoding
    /// the same JSON again until the settings revision trigger reports a change.
    /// The caller must pass the connection supplied to `read`, preserving the
    /// operation's single read transaction.
    pub(crate) fn settings_snapshot(
        &self,
        connection: &Connection,
    ) -> Result<Vec<crate::SettingsDocument>, String> {
        match &self.source {
            ReaderSource::Persistent(readers) => {
                let revision = connection
                    .query_row(
                        "SELECT revision FROM settings_revision WHERE singleton = 1",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(crate::database_error)?;
                let mut cache = readers
                    .settings_snapshot
                    .lock()
                    .map_err(|_| "Settings snapshot cache unavailable".to_string())?;
                if let Some(cached) = cache.as_ref().filter(|cached| cached.revision == revision) {
                    return Ok(cached.documents.clone());
                }
                let documents = crate::persistence::settings::list_settings_documents(connection)?;
                *cache = Some(CachedSettingsSnapshot {
                    revision,
                    documents: documents.clone(),
                });
                Ok(documents)
            }
            #[cfg(any(test, feature = "quality-eval-harness"))]
            ReaderSource::Serialized(_) => {
                crate::persistence::settings::list_settings_documents(connection)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn lane_count(&self) -> usize {
        match self.source {
            ReaderSource::Persistent(_) => READER_LANES,
            #[cfg(any(test, feature = "quality-eval-harness"))]
            ReaderSource::Serialized(_) => 1,
        }
    }

    #[cfg(test)]
    pub(crate) fn cached_settings_revision(&self) -> Option<i64> {
        match &self.source {
            ReaderSource::Persistent(readers) => readers
                .settings_snapshot
                .lock()
                .ok()
                .and_then(|cache| cache.as_ref().map(|cached| cached.revision)),
            ReaderSource::Serialized(_) => None,
        }
    }
}

fn open_reader(database_path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(crate::database_error)?;
    connection
        .execute_batch(
            "PRAGMA query_only=ON;
             PRAGMA foreign_keys=ON;
             PRAGMA busy_timeout=5000;",
        )
        .map_err(crate::database_error)?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    #[test]
    fn a_read_uses_the_free_lane_when_the_round_robin_lane_is_busy() {
        let directory = tempfile::tempdir().expect("temporary directory creates");
        let path = directory.path().join("free-lane.sqlite3");
        let _writer = SqliteWriter::open(&path).expect("writer opens");
        let readers = SqliteReaders::open(&path).expect("readers open");
        let persistent = match &readers.source {
            ReaderSource::Persistent(readers) => readers.clone(),
            ReaderSource::Serialized(_) => unreachable!("fixture uses persistent readers"),
        };
        persistent.next_lane.store(0, Ordering::Relaxed);
        let blocked_lane = persistent.lanes[0].lock().expect("lane lock");
        let (completed, received) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = readers.read(|connection| {
                connection
                    .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
                    .map_err(crate::database_error)
            });
            completed.send(result).expect("result sends");
        });

        assert_eq!(
            received
                .recv_timeout(Duration::from_secs(1))
                .expect("free reader lane completes")
                .expect("read succeeds"),
            1
        );
        drop(blocked_lane);
    }
}
