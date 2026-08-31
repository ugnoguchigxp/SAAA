use super::owner::{DatabaseOwnerGuard, OwnershipError};
use rusqlite::{Connection, Transaction, TransactionBehavior};
#[cfg(test)]
use std::sync::{LockResult, MutexGuard};
use std::{error::Error, fmt, path::Path, sync::Mutex};

pub(crate) struct SqliteWriter {
    connection: Mutex<Connection>,
    _owner: Option<DatabaseOwnerGuard>,
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
        crate::persistence::migrate::backup_before_migration(&connection, database_path)
            .map_err(DatabaseOpenError::Bootstrap)?;
        crate::persistence::schema::initialize_database(&connection)
            .map_err(DatabaseOpenError::Sqlite)?;
        Ok(Self {
            connection: Mutex::new(connection),
            _owner: Some(owner),
        })
    }

    #[cfg(any(test, feature = "quality-eval-harness"))]
    pub(crate) fn from_connection(connection: Connection) -> Self {
        Self {
            connection: Mutex::new(connection),
            _owner: None,
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
        operation(&mut connection)
    }

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
