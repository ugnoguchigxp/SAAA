//! Bounded, read-only coding-job snapshot for the M2A WorldFrame (R4).
//!
//! Only the current run history count, the job row, the current run row and the
//! run source ids are read. Payload, result text, workspace path, session path
//! and PID never leave the coding owner. `repository::authorize` is reused; no
//! cancel / continue operation is issued.

use super::repository;
use crate::database_error;
use rusqlite::{params, Connection};

/// Hard cap on the number of run rows inspected for one job. 32 runs are
/// accepted; a 33rd makes the job's World view omit as `runtime_capacity_omitted`.
pub const MAX_WORLD_RUNS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodingWorldSnapshot {
    pub job_id: String,
    pub conversation_id: String,
    pub source_id: String,
    pub revision: u64,
    pub job_state: String,
    pub current_run_id: String,
    pub run_state: String,
    pub delivery: String,
    pub ended_at: Option<String>,
    pub reported_complete: Option<bool>,
    /// Distinct source ids across the inspected runs, sorted, at most 32.
    pub source_ids: Vec<String>,
}

/// Read the minimal coding owner state. Returns `runtime_capacity_omitted` when
/// the job has more than [`MAX_WORLD_RUNS`] runs so the caller omits the whole
/// job instead of reporting a partial history.
pub fn read_world_snapshot(
    c: &Connection,
    conversation_id: &str,
    job_id: &str,
) -> Result<CodingWorldSnapshot, String> {
    let run_count: usize = c
        .query_row(
            "SELECT COUNT(*) FROM (SELECT 1 FROM coding_runs WHERE job_id=?1 LIMIT 33)",
            [job_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if run_count > MAX_WORLD_RUNS {
        return Err("runtime_capacity_omitted".into());
    }
    repository::authorize(c, job_id, conversation_id)?;
    let (job, conv, source_id, revision, job_state, current_run_id): (
        String,
        String,
        String,
        u64,
        String,
        String,
    ) = c
        .query_row(
            "SELECT id,conversation_id,source_id,revision,state,current_run_id
             FROM coding_jobs WHERE id=?1",
            [job_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .map_err(database_error)?;
    if conv != conversation_id {
        return Err("job_unavailable".into());
    }
    let (run_id, run_state, delivery, ended_at, reported_complete): (
        String,
        String,
        String,
        Option<String>,
        Option<bool>,
    ) = c
        .query_row(
            "SELECT id,state,delivery,ended_at,
                    CASE WHEN json_valid(result_json)
                              AND json_type(result_json,'$.complete') IN ('true','false')
                         THEN json_extract(result_json,'$.complete') END
             FROM coding_runs WHERE id=?1 AND job_id=?2",
            params![current_run_id, job],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .map_err(database_error)?;
    let mut statement = c
        .prepare(
            "SELECT DISTINCT source_id FROM coding_runs
             WHERE job_id=?1 ORDER BY source_id LIMIT 33",
        )
        .map_err(database_error)?;
    let source_ids = statement
        .query_map([&job], |row| row.get::<_, String>(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    if source_ids.len() > MAX_WORLD_RUNS {
        return Err("runtime_capacity_omitted".into());
    }
    Ok(CodingWorldSnapshot {
        job_id: job,
        conversation_id: conv,
        source_id,
        revision,
        job_state,
        current_run_id: run_id,
        run_state,
        delivery,
        ended_at,
        reported_complete,
        source_ids,
    })
}
