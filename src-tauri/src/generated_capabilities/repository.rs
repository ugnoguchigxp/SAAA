use rusqlite::{params, Connection, OptionalExtension};

use super::contracts::{ResolvedCapability, WasmContract};
use super::errors::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RevisionState {
    Candidate,
    Validated,
    Active,
    Suspended,
    Retired,
}

impl RevisionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Validated => "validated",
            Self::Active => "active",
            Self::Suspended => "suspended",
            Self::Retired => "retired",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "candidate" => Self::Candidate,
            "validated" => Self::Validated,
            "active" => Self::Active,
            "suspended" => Self::Suspended,
            "retired" => Self::Retired,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct RevisionRow {
    pub id: String,
    pub capability_id: String,
    pub package_hash: String,
    pub inventory_hash: String,
    pub contract_hash: String,
    pub runtime_digest: String,
    pub required_acceptance_hash: String,
    pub state: RevisionState,
    pub metadata_json: String,
    pub contract_json: String,
    pub manifest_json: String,
    pub provenance_json: String,
}

#[derive(Clone, Debug)]
pub struct CapabilityRow {
    pub id: String,
    pub current_revision_id: Option<String>,
    pub catalog_epoch: i64,
}

#[derive(Clone, Debug)]
pub struct RevisionInsert {
    pub id: String,
    pub capability_id: String,
    pub package_hash: String,
    pub inventory_hash: String,
    pub contract_hash: String,
    pub runtime_digest: String,
    pub required_acceptance_hash: String,
    pub provenance_json: String,
    pub manifest_json: String,
    pub contract_json: String,
    pub metadata_json: String,
    pub created_at: String,
}

#[derive(Clone, Debug)]
pub struct Activation {
    pub capability_id: String,
    pub revision_id: String,
    pub previous_revision_id: Option<String>,
    pub catalog_epoch: i64,
}

pub fn storage(error: rusqlite::Error) -> CapabilityError {
    CapabilityError::new(CapabilityErrorCode::StorageError, error.to_string())
}

fn missing() -> CapabilityError {
    CapabilityError::new(
        CapabilityErrorCode::NotValidated,
        "unknown generated capability revision",
    )
}

const REVISION_COLUMNS: &str = "id, capability_id, package_hash, inventory_hash, contract_hash, \
     runtime_digest, required_acceptance_hash, state, metadata_json, contract_json, manifest_json, \
     provenance_json";

fn revision_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<RevisionRow> {
    let state: String = row.get(7)?;
    Ok(RevisionRow {
        id: row.get(0)?,
        capability_id: row.get(1)?,
        package_hash: row.get(2)?,
        inventory_hash: row.get(3)?,
        contract_hash: row.get(4)?,
        runtime_digest: row.get(5)?,
        required_acceptance_hash: row.get(6)?,
        state: RevisionState::parse(&state).ok_or_else(|| {
            rusqlite::Error::InvalidColumnType(7, state, rusqlite::types::Type::Text)
        })?,
        metadata_json: row.get(8)?,
        contract_json: row.get(9)?,
        manifest_json: row.get(10)?,
        provenance_json: row.get(11)?,
    })
}

pub fn revision_by_id(connection: &Connection, revision_id: &str) -> CapabilityResult<RevisionRow> {
    connection
        .query_row(
            &format!("SELECT {REVISION_COLUMNS} FROM generated_capability_revisions WHERE id = ?1"),
            params![revision_id],
            revision_from,
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(missing)
}

pub fn revision_by_package_hash(
    connection: &Connection,
    capability_id: &str,
    package_hash: &str,
) -> CapabilityResult<Option<RevisionRow>> {
    connection
        .query_row(
            &format!(
                "SELECT {REVISION_COLUMNS} FROM generated_capability_revisions \
                 WHERE capability_id = ?1 AND package_hash = ?2"
            ),
            params![capability_id, package_hash],
            revision_from,
        )
        .optional()
        .map_err(storage)
}

pub fn capability_by_id(
    connection: &Connection,
    capability_id: &str,
) -> CapabilityResult<CapabilityRow> {
    connection
        .query_row(
            "SELECT id, current_revision_id, catalog_epoch FROM generated_capabilities WHERE id = ?1",
            params![capability_id],
            |row| {
                Ok(CapabilityRow {
                    id: row.get(0)?,
                    current_revision_id: row.get(1)?,
                    catalog_epoch: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| {
            CapabilityError::new(CapabilityErrorCode::Conflict, "unknown generated capability")
        })
}

/// Resolves the active revision of a capability. An unknown capability is reported as
/// [`CapabilityErrorCode::Conflict`] (unchanged from M1); a capability that exists but has no
/// active revision returns `None` so callers can skip it without treating it as an error.
pub fn resolve_active(
    connection: &Connection,
    capability_id: &str,
) -> CapabilityResult<Option<ResolvedCapability>> {
    let capability = capability_by_id(connection, capability_id)?;
    let Some(revision_id) = capability.current_revision_id.clone() else {
        return Ok(None);
    };
    let revision = revision_by_id(connection, &revision_id)?;
    if revision.state != RevisionState::Active {
        return Ok(None);
    }
    let contract: WasmContract = serde_json::from_str(&revision.contract_json).map_err(|_| {
        CapabilityError::new(
            CapabilityErrorCode::IntegrityError,
            "stored contract is unreadable",
        )
    })?;
    Ok(Some(ResolvedCapability {
        capability_id: capability.id,
        revision_id: revision.id,
        package_hash: revision.package_hash,
        contract_hash: revision.contract_hash,
        catalog_epoch: capability.catalog_epoch,
        contract,
    }))
}

/// Finds the capability that owns a fixed L-Lang metadata id, creating it when absent.
pub fn ensure_capability(
    connection: &Connection,
    requested_id: Option<&str>,
    metadata_id: &str,
    now: &str,
) -> CapabilityResult<CapabilityRow> {
    if let Some(id) = requested_id {
        return capability_by_id(connection, id);
    }
    let mut statement = connection
        .prepare("SELECT capability_id, metadata_json FROM generated_capability_revisions")
        .map_err(storage)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage)?;
    for row in rows {
        let (capability_id, metadata_json) = row.map_err(storage)?;
        let parsed = serde_json::from_str::<serde_json::Value>(&metadata_json).ok();
        let matches = parsed
            .as_ref()
            .and_then(|value| value.get("id"))
            .and_then(|value| value.as_str())
            == Some(metadata_id);
        if matches {
            return capability_by_id(connection, &capability_id);
        }
    }
    let id = crate::new_id("cap");
    connection
        .execute(
            "INSERT INTO generated_capabilities(id, current_revision_id, catalog_epoch, created_at, updated_at)
             VALUES (?1, NULL, 0, ?2, ?2)",
            params![id, now],
        )
        .map_err(storage)?;
    Ok(CapabilityRow {
        id,
        current_revision_id: None,
        catalog_epoch: 0,
    })
}

pub fn insert_revision(connection: &Connection, revision: &RevisionInsert) -> CapabilityResult<()> {
    connection
        .execute(
            "INSERT INTO generated_capability_revisions(
               id, capability_id, package_hash, inventory_hash, contract_hash, runtime_digest,
               required_acceptance_hash, provenance_json, manifest_json, contract_json,
               metadata_json, state, source_hash, program_hash, artifact_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'candidate', NULL, NULL, NULL, ?12)",
            params![
                revision.id,
                revision.capability_id,
                revision.package_hash,
                revision.inventory_hash,
                revision.contract_hash,
                revision.runtime_digest,
                revision.required_acceptance_hash,
                revision.provenance_json,
                revision.manifest_json,
                revision.contract_json,
                revision.metadata_json,
                revision.created_at,
            ],
        )
        .map_err(storage)?;
    Ok(())
}

pub fn set_revision_state(
    connection: &Connection,
    revision_id: &str,
    state: RevisionState,
) -> CapabilityResult<()> {
    let changed = connection
        .execute(
            "UPDATE generated_capability_revisions SET state = ?2 WHERE id = ?1",
            params![revision_id, state.as_str()],
        )
        .map_err(storage)?;
    if changed != 1 {
        return Err(missing());
    }
    Ok(())
}

pub fn update_runtime_digest(
    connection: &Connection,
    revision_id: &str,
    runtime_digest: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_revisions SET runtime_digest = ?2 WHERE id = ?1",
            params![revision_id, runtime_digest],
        )
        .map_err(storage)?;
    Ok(())
}

pub fn record_hashes(
    connection: &Connection,
    revision_id: &str,
    source_hash: &str,
    program_hash: &str,
    artifact_hash: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_revisions
             SET source_hash = ?2, program_hash = ?3, artifact_hash = ?4 WHERE id = ?1",
            params![revision_id, source_hash, program_hash, artifact_hash],
        )
        .map_err(storage)?;
    Ok(())
}

pub fn has_passed_check(
    connection: &Connection,
    revision_id: &str,
    runtime_digest: &str,
    inventory_hash: &str,
    acceptance_hash: &str,
) -> CapabilityResult<bool> {
    connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM generated_capability_checks
               WHERE revision_id = ?1 AND status = 'passed' AND runtime_digest = ?2
                 AND inventory_hash = ?3 AND acceptance_hash = ?4)",
            params![revision_id, runtime_digest, inventory_hash, acceptance_hash],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage)
}

/// Activates a validated revision inside the caller's transaction.
pub fn activate(
    connection: &Connection,
    revision_id: &str,
    expected_epoch: i64,
    now: &str,
) -> CapabilityResult<Activation> {
    let revision = revision_by_id(connection, revision_id)?;
    if revision.state != RevisionState::Validated {
        return error(
            CapabilityErrorCode::NotValidated,
            "only validated revisions can be activated",
        );
    }
    let capability = capability_by_id(connection, &revision.capability_id)?;
    if capability.catalog_epoch != expected_epoch {
        return error(
            CapabilityErrorCode::Conflict,
            "catalog epoch does not match",
        );
    }
    if !has_passed_check(
        connection,
        revision_id,
        &revision.runtime_digest,
        &revision.inventory_hash,
        &revision.required_acceptance_hash,
    )? {
        return error(
            CapabilityErrorCode::NotValidated,
            "revision has no passed check for the current runtime",
        );
    }
    connection
        .execute(
            "UPDATE generated_capability_revisions SET state = 'validated'
             WHERE capability_id = ?1 AND state = 'active' AND id <> ?2",
            params![revision.capability_id, revision_id],
        )
        .map_err(storage)?;
    connection
        .execute(
            "UPDATE generated_capability_revisions SET state = 'active' WHERE id = ?1",
            params![revision_id],
        )
        .map_err(storage)?;
    let changed = connection
        .execute(
            "UPDATE generated_capabilities
             SET current_revision_id = ?2, catalog_epoch = catalog_epoch + 1, updated_at = ?3
             WHERE id = ?1 AND catalog_epoch = ?4",
            params![revision.capability_id, revision_id, now, expected_epoch],
        )
        .map_err(storage)?;
    if changed != 1 {
        return error(
            CapabilityErrorCode::Conflict,
            "catalog epoch changed during activation",
        );
    }
    Ok(Activation {
        capability_id: revision.capability_id,
        revision_id: revision_id.to_string(),
        previous_revision_id: capability.current_revision_id,
        catalog_epoch: expected_epoch + 1,
    })
}

pub fn suspend(
    connection: &Connection,
    revision_id: &str,
    expected_epoch: i64,
    now: &str,
) -> CapabilityResult<CapabilityRow> {
    let revision = revision_by_id(connection, revision_id)?;
    match revision.state {
        RevisionState::Validated | RevisionState::Active => {}
        RevisionState::Candidate => {
            return error(
                CapabilityErrorCode::NotValidated,
                "candidate revisions cannot be suspended",
            )
        }
        RevisionState::Suspended | RevisionState::Retired => {
            return error(
                CapabilityErrorCode::Conflict,
                "revision is already inactive",
            )
        }
    }
    let capability = capability_by_id(connection, &revision.capability_id)?;
    if capability.catalog_epoch != expected_epoch {
        return error(
            CapabilityErrorCode::Conflict,
            "catalog epoch does not match",
        );
    }
    connection
        .execute(
            "UPDATE generated_capability_revisions SET state = 'suspended' WHERE id = ?1",
            params![revision_id],
        )
        .map_err(storage)?;
    let clears_pointer = capability.current_revision_id.as_deref() == Some(revision_id);
    let changed = connection
        .execute(
            "UPDATE generated_capabilities
             SET current_revision_id = CASE WHEN current_revision_id = ?2 THEN NULL ELSE current_revision_id END,
                 catalog_epoch = catalog_epoch + 1, updated_at = ?3
             WHERE id = ?1 AND catalog_epoch = ?4",
            params![
                revision.capability_id,
                revision_id,
                now,
                expected_epoch
            ],
        )
        .map_err(storage)?;
    if changed != 1 {
        return error(
            CapabilityErrorCode::Conflict,
            "catalog epoch changed during suspension",
        );
    }
    Ok(CapabilityRow {
        id: revision.capability_id,
        current_revision_id: if clears_pointer {
            None
        } else {
            capability.current_revision_id
        },
        catalog_epoch: expected_epoch + 1,
    })
}

pub fn insert_import(connection: &Connection, import_id: &str, now: &str) -> CapabilityResult<()> {
    connection
        .execute(
            "INSERT INTO generated_capability_imports(id, status, revision_id, error_code, created_at, completed_at)
             VALUES (?1, 'staging', NULL, NULL, ?2, NULL)",
            params![import_id, now],
        )
        .map_err(storage)?;
    Ok(())
}

pub fn finish_import(
    connection: &Connection,
    import_id: &str,
    status: &str,
    revision_id: Option<&str>,
    error_code: Option<&str>,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_imports
             SET status = ?2, revision_id = ?3, error_code = ?4, completed_at = ?5
             WHERE id = ?1",
            params![import_id, status, revision_id, error_code, now],
        )
        .map_err(storage)?;
    Ok(())
}

pub fn insert_check(
    connection: &Connection,
    check_id: &str,
    revision_id: &str,
    runtime_digest: &str,
    inventory_hash: &str,
    acceptance_hash: &str,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "INSERT INTO generated_capability_checks(
               id, revision_id, runtime_digest, inventory_hash, acceptance_hash, status,
               error_code, report_ref, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'running', NULL, NULL, ?6, NULL)",
            params![
                check_id,
                revision_id,
                runtime_digest,
                inventory_hash,
                acceptance_hash,
                now
            ],
        )
        .map_err(storage)?;
    Ok(())
}

pub fn finish_check(
    connection: &Connection,
    check_id: &str,
    status: &str,
    error_code: Option<&str>,
    report_ref: Option<&str>,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_checks
             SET status = ?2, error_code = ?3, report_ref = ?4, completed_at = ?5
             WHERE id = ?1",
            params![check_id, status, error_code, report_ref, now],
        )
        .map_err(storage)?;
    Ok(())
}

pub fn running_check_exists(connection: &Connection, revision_id: &str) -> CapabilityResult<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM generated_capability_checks WHERE revision_id = ?1 AND status = 'running')",
            params![revision_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage)
}

pub fn insert_call(
    connection: &Connection,
    call_id: &str,
    revision_id: &str,
    package_hash: &str,
    origin: &str,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "INSERT INTO generated_capability_calls(
               id, revision_id, package_hash, origin, status, result_bool, error_code, started_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, 'running', NULL, NULL, ?5, NULL)",
            params![call_id, revision_id, package_hash, origin, now],
        )
        .map_err(storage)?;
    Ok(())
}

pub fn finish_call(
    connection: &Connection,
    call_id: &str,
    status: &str,
    result: Option<bool>,
    error_code: Option<&str>,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_calls
             SET status = ?2, result_bool = ?3, error_code = ?4, completed_at = ?5
             WHERE id = ?1",
            params![call_id, status, result, error_code, now],
        )
        .map_err(storage)?;
    Ok(())
}

/// Moves one check to `interrupted` only if it is still `running`.
pub fn interrupt_check(
    connection: &Connection,
    check_id: &str,
    now: &str,
) -> CapabilityResult<usize> {
    connection
        .execute(
            "UPDATE generated_capability_checks SET status = 'interrupted', completed_at = ?2
             WHERE id = ?1 AND status = 'running'",
            params![check_id, now],
        )
        .map_err(storage)
}

/// Moves one import to `interrupted` only if it is still `staging`.
pub fn interrupt_import(
    connection: &Connection,
    import_id: &str,
    now: &str,
) -> CapabilityResult<usize> {
    connection
        .execute(
            "UPDATE generated_capability_imports SET status = 'interrupted', completed_at = ?2
             WHERE id = ?1 AND status = 'staging'",
            params![import_id, now],
        )
        .map_err(storage)
}

/// Counts records that never reached a terminal status.
pub fn unsettled_rows(connection: &Connection) -> CapabilityResult<i64> {
    connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM generated_capability_calls WHERE status = 'running')
                  + (SELECT COUNT(*) FROM generated_capability_checks WHERE status = 'running')
                  + (SELECT COUNT(*) FROM generated_capability_imports WHERE status = 'staging')",
            [],
            |row| row.get(0),
        )
        .map_err(storage)
}

/// Recovery: any check/call/import left running belongs to a previous process.
pub fn interrupt_running(connection: &Connection) -> CapabilityResult<(usize, usize, usize)> {
    let now = crate::now_iso();
    let checks = connection
        .execute(
            "UPDATE generated_capability_checks SET status = 'interrupted', completed_at = ?1
             WHERE status = 'running'",
            params![now],
        )
        .map_err(storage)?;
    let calls = connection
        .execute(
            "UPDATE generated_capability_calls SET status = 'interrupted', completed_at = ?1
             WHERE status = 'running'",
            params![now],
        )
        .map_err(storage)?;
    let imports = connection
        .execute(
            "UPDATE generated_capability_imports SET status = 'interrupted', completed_at = ?1
             WHERE status = 'staging'",
            params![now],
        )
        .map_err(storage)?;
    Ok((checks, calls, imports))
}

/// Recovery: an active revision whose pointer disagrees, or whose package is gone, is stopped.
pub fn stop_inconsistent_capabilities(
    connection: &Connection,
    now: &str,
) -> CapabilityResult<Vec<String>> {
    let mut inconsistent: Vec<String> = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT c.id FROM generated_capabilities c
                 LEFT JOIN generated_capability_revisions r
                   ON r.id = c.current_revision_id AND r.capability_id = c.id
                 WHERE c.current_revision_id IS NOT NULL
                   AND (r.id IS NULL OR r.state <> 'active')",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage)?;
        for row in rows {
            inconsistent.push(row.map_err(storage)?);
        }
    }
    {
        let mut statement = connection
            .prepare(
                "SELECT capability_id FROM generated_capability_revisions WHERE state = 'active'",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(storage)?;
        for row in rows {
            let capability_id = row.map_err(storage)?;
            let capability = capability_by_id(connection, &capability_id)?;
            let matched = capability
                .current_revision_id
                .as_deref()
                .map(|id| {
                    connection
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM generated_capability_revisions
                              WHERE id = ?1 AND capability_id = ?2 AND state = 'active')",
                            params![id, capability_id],
                            |row| row.get::<_, bool>(0),
                        )
                        .map_err(storage)
                })
                .transpose()?
                .unwrap_or(false);
            if !matched {
                inconsistent.push(capability_id);
            }
        }
    }
    inconsistent.sort();
    inconsistent.dedup();
    for capability_id in &inconsistent {
        connection
            .execute(
                "UPDATE generated_capability_revisions SET state = 'suspended'
                 WHERE capability_id = ?1 AND state = 'active'",
                params![capability_id],
            )
            .map_err(storage)?;
        connection
            .execute(
                "UPDATE generated_capabilities
                 SET current_revision_id = NULL, catalog_epoch = catalog_epoch + 1, updated_at = ?2
                 WHERE id = ?1",
                params![capability_id, now],
            )
            .map_err(storage)?;
    }
    Ok(inconsistent)
}

pub fn package_hashes_in_use(connection: &Connection) -> CapabilityResult<Vec<String>> {
    let mut statement = connection
        .prepare("SELECT DISTINCT package_hash FROM generated_capability_revisions")
        .map_err(storage)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage)?;
    let mut hashes = Vec::new();
    for row in rows {
        hashes.push(row.map_err(storage)?);
    }
    Ok(hashes)
}

#[derive(Clone, Debug)]
pub struct ActiveRevision {
    pub revision_id: String,
    pub capability_id: String,
    pub package_hash: String,
    pub inventory_hash: String,
}

pub fn active_revisions(connection: &Connection) -> CapabilityResult<Vec<ActiveRevision>> {
    let mut statement = connection
        .prepare(
            "SELECT id, capability_id, package_hash, inventory_hash
             FROM generated_capability_revisions WHERE state = 'active'",
        )
        .map_err(storage)?;
    let rows = statement
        .query_map([], |row| {
            Ok(ActiveRevision {
                revision_id: row.get(0)?,
                capability_id: row.get(1)?,
                package_hash: row.get(2)?,
                inventory_hash: row.get(3)?,
            })
        })
        .map_err(storage)?;
    let mut revisions = Vec::new();
    for row in rows {
        revisions.push(row.map_err(storage)?);
    }
    Ok(revisions)
}

/// Stops a capability whose managed package is unusable: state to suspended, pointer cleared,
/// epoch advanced. Recovery never guesses a replacement revision.
pub fn stop_capability(
    connection: &Connection,
    capability_id: &str,
    now: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_revisions SET state = 'suspended'
             WHERE capability_id = ?1 AND state = 'active'",
            params![capability_id],
        )
        .map_err(storage)?;
    connection
        .execute(
            "UPDATE generated_capabilities
             SET current_revision_id = NULL, catalog_epoch = catalog_epoch + 1, updated_at = ?2
             WHERE id = ?1",
            params![capability_id, now],
        )
        .map_err(storage)?;
    Ok(())
}
