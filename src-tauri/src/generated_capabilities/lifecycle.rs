use rusqlite::TransactionBehavior;

use super::{
    errors::*,
    repository::{self, RevisionState},
};
use crate::{now_iso, persistence::SqliteWriter};

/// Runs a short synchronous transaction through the single writer. No subprocess wait, file
/// copy, network call or `.await` may happen inside `action`.
pub(crate) fn transaction<T>(
    writer: &SqliteWriter,
    action: impl FnOnce(&rusqlite::Transaction<'_>) -> CapabilityResult<T>,
) -> CapabilityResult<T> {
    writer
        .write(|connection| {
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| {
                    CapabilityError::new(CapabilityErrorCode::StorageError, error.to_string())
                        .encode()
                })?;
            let value = action(&transaction).map_err(|error| error.encode())?;
            transaction.commit().map_err(|error| {
                CapabilityError::new(CapabilityErrorCode::StorageError, error.to_string()).encode()
            })?;
            Ok(value)
        })
        .map_err(CapabilityError::decode)
}

/// Suspends a validated or active revision at the expected catalog epoch.
pub(crate) fn suspend_revision(
    writer: &SqliteWriter,
    revision_id: &str,
    expected_epoch: i64,
) -> CapabilityResult<repository::CapabilityRow> {
    let now = now_iso();
    transaction(writer, |transaction| {
        let capability = repository::suspend(transaction, revision_id, expected_epoch, &now)?;
        super::publication_sync::unpublish_catalog_for_capability(transaction, &capability.id)?;
        Ok(capability)
    })
}

/// Retires a non-active revision at the expected catalog epoch. Call history and inspections are
/// retained; only new selection and execution are prevented.
pub(crate) fn retire_revision(
    writer: &SqliteWriter,
    revision_id: &str,
    expected_epoch: i64,
) -> CapabilityResult<repository::CapabilityRow> {
    let now = now_iso();
    transaction(writer, |transaction| {
        super::retirement::retire(transaction, revision_id, expected_epoch, &now)
    })
}

/// Runs a short synchronous read through the single writer connection.
pub(crate) fn read<T>(
    writer: &SqliteWriter,
    action: impl FnOnce(&rusqlite::Connection) -> CapabilityResult<T>,
) -> CapabilityResult<T> {
    writer
        .read_serialized(|connection| action(connection).map_err(|error| error.encode()))
        .map_err(CapabilityError::decode)
}

pub fn state_is_verifiable(state: RevisionState) -> bool {
    matches!(state, RevisionState::Candidate | RevisionState::Validated)
}
