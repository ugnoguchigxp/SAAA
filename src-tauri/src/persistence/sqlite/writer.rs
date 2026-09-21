use super::owner::{DatabaseOwnerGuard, OwnershipError};
use crate::memory::personal_state::journal;
use rusqlite::Connection;
#[cfg(test)]
use rusqlite::{Transaction, TransactionBehavior};
#[cfg(test)]
use std::sync::{LockResult, MutexGuard};
use std::{error::Error, fmt, path::Path, sync::Mutex};

pub(crate) struct SqliteWriter {
    connection: Mutex<Connection>,
    _owner: Option<DatabaseOwnerGuard>,
    forget_journal: Option<Mutex<journal::Journal>>,
}

#[derive(Debug)]
pub(crate) enum DatabaseOpenError {
    AlreadyOwned,
    OwnershipUnavailable,
    Sqlite(rusqlite::Error),
    Bootstrap(String),
}

impl fmt::Display for DatabaseOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyOwned => formatter.write_str("database-already-owned"),
            Self::OwnershipUnavailable => formatter.write_str("database-ownership-unavailable"),
            Self::Sqlite(error) => write!(formatter, "Could not open the database: {error}"),
            Self::Bootstrap(error) => formatter.write_str(error),
        }
    }
}

impl Error for DatabaseOpenError {}

impl SqliteWriter {
    pub(crate) fn open(database_path: &Path) -> Result<Self, DatabaseOpenError> {
        let owner = DatabaseOwnerGuard::acquire(database_path).map_err(|error| match error {
            OwnershipError::AlreadyOwned => DatabaseOpenError::AlreadyOwned,
            OwnershipError::Unavailable(_error) => DatabaseOpenError::OwnershipUnavailable,
        })?;
        let connection = Connection::open(database_path).map_err(DatabaseOpenError::Sqlite)?;
        let previous_version =
            journal::database_version(&connection).map_err(DatabaseOpenError::Sqlite)?;
        crate::persistence::migrate::backup_before_migration(&connection, database_path)
            .map_err(DatabaseOpenError::Bootstrap)?;
        crate::persistence::schema::initialize_database(&connection)
            .map_err(DatabaseOpenError::Sqlite)?;
        let journal = journal::open_database(&connection, database_path, previous_version)
            .map_err(DatabaseOpenError::Bootstrap)?;
        Ok(Self {
            forget_journal: Some(Mutex::new(journal)),
            connection: Mutex::new(connection),
            _owner: Some(owner),
        })
    }

    #[cfg(any(test, feature = "quality-eval-harness"))]
    pub(crate) fn from_connection(connection: Connection) -> Self {
        Self {
            connection: Mutex::new(connection),
            _owner: None,
            forget_journal: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn lock(&self) -> LockResult<MutexGuard<'_, Connection>> {
        self.connection.lock()
    }

    pub(crate) fn write<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "Database writer unavailable".to_string())?;
        let result = operation(&mut connection);
        journal::sync_locked(&self.forget_journal, &connection)?;
        result
    }

    /// Commits `operation` as one SQLite transaction. Callers must not perform
    /// network or process I/O inside the closure, and must not re-enter the writer.
    pub(crate) fn transact<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        self.write(|connection| {
            let transaction = connection
                .unchecked_transaction()
                .map_err(crate::database_error)?;
            match operation(&transaction) {
                Ok(value) => {
                    transaction.commit().map_err(crate::database_error)?;
                    Ok(value)
                }
                Err(error) => Err(error),
            }
        })
    }

    #[cfg(test)]
    pub(crate) fn write_transaction<T>(
        &self,
        behavior: TransactionBehavior,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, String>,
    ) -> Result<T, String> {
        self.write(|connection| {
            let transaction = connection
                .transaction_with_behavior(behavior)
                .map_err(crate::database_error)?;
            let result = operation(&transaction)?;
            transaction.commit().map_err(crate::database_error)?;
            Ok(result)
        })
    }

    pub(crate) fn read_serialized<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| "Database writer unavailable".to_string())?;
        operation(&connection)
    }
}
