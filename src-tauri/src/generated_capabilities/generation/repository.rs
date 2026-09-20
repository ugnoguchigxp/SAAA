//! Persistence for generation jobs, call ownership and inspections (plan 12.4, 12.6).
//!
//! Every status change is a compare-and-swap on `(id, status)` so a late cancellation can never
//! rewrite an already committed terminal row. Times on the new tables are unix milliseconds.

use rusqlite::{params, Connection, OptionalExtension};

use super::super::errors::*;
use super::contracts::{GenerationErrorCode, GenerationStatus};

pub fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
pub struct NewGenerationJob {
    pub id: String,
    pub principal_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub input_message_id: String,
    pub project_id: Option<String>,
    pub request_id: String,
    pub request_snapshot_json: String,
    pub request_digest: String,
    pub capability_id: String,
    pub base_revision_id: Option<String>,
    pub expected_epoch: i64,
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct GenerationJobRow {
    pub id: String,
    pub principal_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub input_message_id: String,
    pub project_id: Option<String>,
    pub request_id: String,
    pub request_snapshot_json: String,
    pub request_digest: String,
    pub capability_id: String,
    pub base_revision_id: Option<String>,
    pub expected_epoch: i64,
    pub revision_id: Option<String>,
    pub status: GenerationStatus,
    pub error_code: Option<String>,
    pub usage_json: Option<String>,
    pub created_at: i64,
    pub completed_at: Option<i64>,
}

const JOB_COLUMNS: &str =
    "id, principal_id, conversation_id, run_id, input_message_id, project_id, \
     request_id, request_snapshot_json, request_digest, capability_id, base_revision_id, \
     expected_epoch, revision_id, status, error_code, usage_json, created_at, completed_at";

fn job_from(row: &rusqlite::Row<'_>) -> rusqlite::Result<GenerationJobRow> {
    let status: String = row.get(13)?;
    Ok(GenerationJobRow {
        id: row.get(0)?,
        principal_id: row.get(1)?,
        conversation_id: row.get(2)?,
        run_id: row.get(3)?,
        input_message_id: row.get(4)?,
        project_id: row.get(5)?,
        request_id: row.get(6)?,
        request_snapshot_json: row.get(7)?,
        request_digest: row.get(8)?,
        capability_id: row.get(9)?,
        base_revision_id: row.get(10)?,
        expected_epoch: row.get(11)?,
        revision_id: row.get(12)?,
        status: GenerationStatus::parse(&status).ok_or_else(|| {
            rusqlite::Error::InvalidColumnType(13, status, rusqlite::types::Type::Text)
        })?,
        error_code: row.get(14)?,
        usage_json: row.get(15)?,
        created_at: row.get(16)?,
        completed_at: row.get(17)?,
    })
}

#[derive(Clone, Debug)]
pub struct CallRow {
    pub id: String,
    pub revision_id: String,
    pub package_hash: String,
    pub origin: String,
    pub status: String,
}

pub fn call_by_id(connection: &Connection, call_id: &str) -> CapabilityResult<Option<CallRow>> {
    connection
        .query_row(
            "SELECT id, revision_id, package_hash, origin, status \
             FROM generated_capability_calls WHERE id = ?1",
            params![call_id],
            |row| {
                Ok(CallRow {
                    id: row.get(0)?,
                    revision_id: row.get(1)?,
                    package_hash: row.get(2)?,
                    origin: row.get(3)?,
                    status: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(storage)
}

pub fn insert_job(connection: &Connection, job: &NewGenerationJob) -> CapabilityResult<()> {
    connection
        .execute(
            "INSERT INTO generated_capability_generation_jobs(
               id, principal_id, conversation_id, run_id, input_message_id, project_id,
               request_id, request_snapshot_json, request_digest, capability_id,
               base_revision_id, expected_epoch, revision_id, status, error_code, usage_json,
               created_at, completed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, NULL, 'requested',
                     NULL, NULL, ?13, NULL)",
            params![
                job.id,
                job.principal_id,
                job.conversation_id,
                job.run_id,
                job.input_message_id,
                job.project_id,
                job.request_id,
                job.request_snapshot_json,
                job.request_digest,
                job.capability_id,
                job.base_revision_id,
                job.expected_epoch,
                job.created_at,
            ],
        )
        .map_err(|error| {
            let text = error.to_string();
            let code = if text.contains("UNIQUE") {
                CapabilityErrorCode::Conflict
            } else {
                CapabilityErrorCode::StorageError
            };
            CapabilityError::new(code, "could not record the generation job")
        })?;
    Ok(())
}

pub fn job_by_id(connection: &Connection, job_id: &str) -> CapabilityResult<GenerationJobRow> {
    connection
        .query_row(
            &format!(
                "SELECT {JOB_COLUMNS} FROM generated_capability_generation_jobs WHERE id = ?1"
            ),
            params![job_id],
            job_from,
        )
        .optional()
        .map_err(storage)?
        .ok_or_else(|| {
            CapabilityError::new(CapabilityErrorCode::NotValidated, "unknown generation job")
        })
}

pub fn job_by_identity(
    connection: &Connection,
    principal_id: &str,
    run_id: &str,
    input_message_id: &str,
) -> CapabilityResult<Option<GenerationJobRow>> {
    connection
        .query_row(
            &format!(
                "SELECT {JOB_COLUMNS} FROM generated_capability_generation_jobs
                 WHERE principal_id = ?1 AND run_id = ?2 AND input_message_id = ?3"
            ),
            params![principal_id, run_id, input_message_id],
            job_from,
        )
        .optional()
        .map_err(storage)
}

pub fn active_job_for_capability(
    connection: &Connection,
    capability_id: &str,
) -> CapabilityResult<Option<GenerationJobRow>> {
    connection
        .query_row(
            &format!(
                "SELECT {JOB_COLUMNS} FROM generated_capability_generation_jobs
                 WHERE capability_id = ?1
                   AND status IN ('requested','generating','building','importing','verifying',
                                  'awaiting_activation')"
            ),
            params![capability_id],
            job_from,
        )
        .optional()
        .map_err(storage)
}

/// Compare-and-swap: exactly one row must move from `from` to `to`.
pub fn cas_status(
    connection: &Connection,
    job_id: &str,
    from: GenerationStatus,
    to: GenerationStatus,
    now: i64,
) -> CapabilityResult<bool> {
    let terminal = to.is_terminal();
    let changed = connection
        .execute(
            "UPDATE generated_capability_generation_jobs
             SET status = ?3,
                 completed_at = CASE WHEN ?4 THEN ?5 ELSE completed_at END
             WHERE id = ?1 AND status = ?2",
            params![job_id, from.as_str(), to.as_str(), terminal, now],
        )
        .map_err(storage)?;
    Ok(changed == 1)
}

/// Terminal transition that also records the fixed error code and optional usage.
pub fn finish(
    connection: &Connection,
    job_id: &str,
    from: GenerationStatus,
    status: GenerationStatus,
    error_code: Option<GenerationErrorCode>,
    usage_json: Option<&str>,
    now: i64,
) -> CapabilityResult<bool> {
    if !status.is_terminal() {
        return error(
            CapabilityErrorCode::StorageError,
            "finish requires a terminal status",
        );
    }
    let changed = connection
        .execute(
            "UPDATE generated_capability_generation_jobs
             SET status = ?3, error_code = ?4, usage_json = COALESCE(?5, usage_json), completed_at = ?6
             WHERE id = ?1 AND status = ?2",
            params![
                job_id,
                from.as_str(),
                status.as_str(),
                error_code.map(|code| code.as_str()),
                usage_json,
                now
            ],
        )
        .map_err(storage)?;
    Ok(changed == 1)
}

pub fn set_revision(
    connection: &Connection,
    job_id: &str,
    revision_id: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_generation_jobs SET revision_id = ?2 WHERE id = ?1",
            params![job_id, revision_id],
        )
        .map_err(storage)?;
    Ok(())
}

pub fn set_usage(connection: &Connection, job_id: &str, usage_json: &str) -> CapabilityResult<()> {
    connection
        .execute(
            "UPDATE generated_capability_generation_jobs SET usage_json = ?2 WHERE id = ?1",
            params![job_id, usage_json],
        )
        .map_err(storage)?;
    Ok(())
}

/// Recovery moves jobs interrupted mid-generation to `interrupted`. `awaiting_activation` is a
/// verified candidate and is deliberately kept. Returns how many rows were changed.
pub fn interrupt_running(connection: &Connection, now: i64) -> CapabilityResult<usize> {
    connection
        .execute(
            "UPDATE generated_capability_generation_jobs
             SET status = 'interrupted', completed_at = ?1
             WHERE status IN ('requested','generating','building','importing','verifying')",
            params![now],
        )
        .map_err(storage)
}

// -- call ownership -----------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallOwnerRow {
    pub call_id: String,
    pub principal_id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub run_id: String,
}

/// Records the owner of one generated-capability call. Called in the same transaction that
/// inserts the call row; a call without an owner is never treated as owned by anyone.
pub fn insert_call_owner(
    connection: &Connection,
    call_id: &str,
    principal_id: &str,
    conversation_id: &str,
    project_id: Option<&str>,
    run_id: &str,
) -> CapabilityResult<()> {
    connection
        .execute(
            "INSERT INTO generated_capability_call_owners(
               call_id, principal_id, conversation_id, project_id, run_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![call_id, principal_id, conversation_id, project_id, run_id],
        )
        .map_err(|_| {
            CapabilityError::new(
                CapabilityErrorCode::StorageError,
                "could not record the call owner",
            )
        })?;
    Ok(())
}

/// Convenience wrapper used by invocation so the owner write stays one line at the call site.
pub fn insert_call_owner_for(
    connection: &Connection,
    call_id: &str,
    actor: &crate::generated_capabilities::contracts::CallActor,
) -> CapabilityResult<()> {
    insert_call_owner(
        connection,
        call_id,
        &actor.principal_id,
        &actor.conversation_id,
        actor.project_id.as_deref(),
        &actor.run_id,
    )
}

pub fn call_owner(
    connection: &Connection,
    call_id: &str,
) -> CapabilityResult<Option<CallOwnerRow>> {
    connection
        .query_row(
            "SELECT call_id, principal_id, conversation_id, project_id, run_id
             FROM generated_capability_call_owners WHERE call_id = ?1",
            params![call_id],
            |row| {
                Ok(CallOwnerRow {
                    call_id: row.get(0)?,
                    principal_id: row.get(1)?,
                    conversation_id: row.get(2)?,
                    project_id: row.get(3)?,
                    run_id: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(storage)
}

fn storage(error: rusqlite::Error) -> CapabilityError {
    CapabilityError::new(CapabilityErrorCode::StorageError, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::schema::initialize_database;
    use rusqlite::Connection;

    fn job(
        id: &str,
        principal: &str,
        run: &str,
        message: &str,
        capability: &str,
    ) -> NewGenerationJob {
        NewGenerationJob {
            id: id.into(),
            principal_id: principal.into(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            run_id: run.into(),
            input_message_id: message.into(),
            project_id: None,
            request_id: "req-1".into(),
            request_snapshot_json: "{}".into(),
            request_digest: "a".repeat(64),
            capability_id: capability.into(),
            base_revision_id: None,
            expected_epoch: 0,
            created_at: unix_ms(),
        }
    }

    #[test]
    fn job_identity_is_idempotent_and_cas_holds() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        insert_job(&connection, &job("job-1", "P1", "run-1", "msg-1", "cap-1")).unwrap();
        assert!(insert_job(&connection, &job("job-1", "P1", "run-1", "msg-1", "cap-1")).is_err());
        let found = job_by_identity(&connection, "P1", "run-1", "msg-1")
            .unwrap()
            .unwrap();
        assert_eq!(found.id, "job-1");
        assert_eq!(found.status, GenerationStatus::Requested);

        assert!(cas_status(
            &connection,
            "job-1",
            GenerationStatus::Requested,
            GenerationStatus::Generating,
            unix_ms()
        )
        .unwrap());
        // Replaying the same transition is rejected, not applied twice.
        assert!(!cas_status(
            &connection,
            "job-1",
            GenerationStatus::Requested,
            GenerationStatus::Generating,
            unix_ms()
        )
        .unwrap());
        assert!(finish(
            &connection,
            "job-1",
            GenerationStatus::Generating,
            GenerationStatus::Failed,
            Some(GenerationErrorCode::ModelError),
            None,
            unix_ms()
        )
        .unwrap());
        let done = job_by_id(&connection, "job-1").unwrap();
        assert_eq!(done.status, GenerationStatus::Failed);
        assert_eq!(done.error_code.as_deref(), Some("model-error"));
    }

    #[test]
    fn one_active_job_per_capability() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        insert_job(&connection, &job("job-1", "P1", "run-1", "msg-1", "cap-1")).unwrap();
        assert!(insert_job(&connection, &job("job-2", "P2", "run-2", "msg-2", "cap-1")).is_err());
        let busy = active_job_for_capability(&connection, "cap-1")
            .unwrap()
            .unwrap();
        assert_eq!(busy.id, "job-1");
        cas_status(
            &connection,
            "job-1",
            GenerationStatus::Requested,
            GenerationStatus::Cancelled,
            unix_ms(),
        )
        .unwrap();
        assert!(active_job_for_capability(&connection, "cap-1")
            .unwrap()
            .is_none());
        insert_job(&connection, &job("job-3", "P2", "run-2", "msg-2", "cap-1")).unwrap();
    }

    #[test]
    fn foreign_keys_reject_unknown_conversation_and_call() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        let mut bad = job("job-bad", "P1", "run-bad", "msg-bad", "cap-bad");
        bad.conversation_id = "does-not-exist".into();
        assert!(insert_job(&connection, &bad).is_err());
        // A call owner for an unknown call is refused by the FK.
        assert!(insert_call_owner(
            &connection,
            "missing-call",
            "P1",
            crate::PRIMARY_CONVERSATION_ID,
            None,
            "run-1",
        )
        .is_err());
    }

    #[test]
    fn call_owners_are_recorded_per_call() {
        let connection = Connection::open_in_memory().unwrap();
        initialize_database(&connection).unwrap();
        // A call row is required by the owner FK.
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
                 VALUES('r','c','p','i','c','d','a','{}','{}','{}','{}','candidate','x')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO generated_capability_calls(
                   id, revision_id, package_hash, origin, status, started_at)
                 VALUES('call-1','r','p','conversation','running','x')",
                [],
            )
            .unwrap();
        insert_call_owner(
            &connection,
            "call-1",
            "P1",
            crate::PRIMARY_CONVERSATION_ID,
            None,
            "run-1",
        )
        .unwrap();
        let owner = call_owner(&connection, "call-1").unwrap().unwrap();
        assert_eq!(owner.principal_id, "P1");
        assert!(call_owner(&connection, "call-unknown").unwrap().is_none());
    }
}
