//! Revision retirement (plan 4, C08). Retirement removes a non-active revision from new
//! selection and execution but never deletes its package, call history or inspections.

use rusqlite::params;

use super::errors::{error, CapabilityErrorCode, CapabilityResult};
use super::repository::{self, RevisionState};

/// Retires a non-active revision. Retiring removes it from new selection and execution but never
/// deletes its package, call history or inspections. An active revision must be suspended first.
pub fn retire(
    connection: &rusqlite::Connection,
    revision_id: &str,
    expected_epoch: i64,
    now: &str,
) -> CapabilityResult<repository::CapabilityRow> {
    let revision = repository::revision_by_id(connection, revision_id)?;
    match revision.state {
        RevisionState::Active => {
            return error(
                CapabilityErrorCode::Conflict,
                "an active revision must be suspended before it can be retired",
            )
        }
        RevisionState::Retired => {
            return error(CapabilityErrorCode::Conflict, "revision is already retired")
        }
        RevisionState::Candidate | RevisionState::Validated | RevisionState::Suspended => {}
    }
    let capability = repository::capability_by_id(connection, &revision.capability_id)?;
    if capability.catalog_epoch != expected_epoch {
        return error(
            CapabilityErrorCode::Conflict,
            "catalog epoch does not match",
        );
    }
    connection
        .execute(
            "UPDATE generated_capability_revisions SET state = 'retired' WHERE id = ?1",
            params![revision_id],
        )
        .map_err(repository::storage)?;
    let changed = connection
        .execute(
            "UPDATE generated_capabilities
             SET catalog_epoch = catalog_epoch + 1, updated_at = ?2
             WHERE id = ?1 AND catalog_epoch = ?3",
            params![revision.capability_id, now, expected_epoch],
        )
        .map_err(repository::storage)?;
    if changed != 1 {
        return error(
            CapabilityErrorCode::Conflict,
            "catalog epoch changed during retirement",
        );
    }
    Ok(repository::CapabilityRow {
        id: revision.capability_id,
        current_revision_id: capability.current_revision_id,
        catalog_epoch: expected_epoch + 1,
    })
}

/// Stops a capability whose managed package is unusable: state to suspended, pointer cleared,
/// epoch advanced. Recovery never guesses a replacement revision.
pub fn stop_capability(
    connection: &rusqlite::Connection,
    capability_id: &str,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_revisions SET state = 'suspended'
             WHERE capability_id = ?1 AND state = 'active'",
            params![capability_id],
        )
        .map_err(repository::storage)?;
    connection
        .execute(
            "UPDATE generated_capabilities
             SET current_revision_id = NULL, catalog_epoch = catalog_epoch + 1, updated_at = ?2
             WHERE id = ?1",
            params![capability_id, now],
        )
        .map_err(repository::storage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated_capabilities::errors::CapabilityErrorCode;
    use crate::generated_capabilities::repository::{self, revision_by_id, RevisionState};
    use crate::persistence::schema::initialize_database;
    use rusqlite::params;

    fn seed(connection: &rusqlite::Connection, state: &str) {
        initialize_database(connection).unwrap();
        connection
            .execute(
                "INSERT INTO generated_capabilities(id, created_at, updated_at) VALUES('c','x','x')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO generated_capability_revisions(
                   id, capability_id, package_hash, inventory_hash, contract_hash, runtime_digest,
                   required_acceptance_hash, provenance_json, manifest_json, contract_json,
                   metadata_json, state, created_at)
                 VALUES('r','c','p','i','c','d','a','{}','{}','{}','{}',?1,'x')",
                params![state],
            )
            .unwrap();
    }

    #[test]
    fn active_revision_retire_is_refused_until_suspended() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        seed(&connection, "active");
        connection
            .execute(
                "UPDATE generated_capabilities SET current_revision_id='r' WHERE id='c'",
                [],
            )
            .unwrap();
        assert_eq!(
            retire(&connection, "r", 0, "now").unwrap_err().code,
            CapabilityErrorCode::Conflict
        );
        let suspended = repository::suspend(&connection, "r", 0, "now").unwrap();
        assert_eq!(suspended.catalog_epoch, 1);
        let retired = retire(&connection, "r", 1, "now").unwrap();
        assert_eq!(retired.catalog_epoch, 2);
        assert_eq!(
            revision_by_id(&connection, "r").unwrap().state,
            RevisionState::Retired
        );
        // The package/revision row is retained, and a second retire conflicts.
        assert!(revision_by_id(&connection, "r").is_ok());
        assert!(retire(&connection, "r", 2, "now").is_err());
    }

    #[test]
    fn stale_epoch_retire_conflicts_without_changing_state() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        seed(&connection, "validated");
        assert_eq!(
            retire(&connection, "r", 5, "now").unwrap_err().code,
            CapabilityErrorCode::Conflict
        );
        assert_eq!(
            revision_by_id(&connection, "r").unwrap().state,
            RevisionState::Validated
        );
    }
}
