//! Budget reservation, consumption, and remaining deadline.
use super::repository::ActiveWork;
use crate::database_error;
use rusqlite::{params, Connection, OptionalExtension};

pub(crate) fn remaining_deadline_ms(
    connection: &Connection,
    work: &ActiveWork,
) -> Result<i64, String> {
    let consumed: i64 = connection
        .query_row(
            "SELECT COALESCE(SUM(
                CAST((julianday(COALESCE(r.ended_at, ?2)) - julianday(r.started_at)) * 86400000 AS INTEGER)
             ),0)
             FROM coding_runs r
             JOIN coding_jobs j ON j.id=r.job_id
             JOIN steward_tasks t ON t.coding_job_id=j.id
             WHERE t.delegation_id=?1",
            params![work.delegation_id, crate::now_iso()],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?
        .unwrap_or(0);
    Ok((work.budget_ms - consumed).max(1))
}

pub(crate) fn release_unsent(connection: &Connection, task_id: &str) -> Result<(), String> {
    connection
        .execute(
            "UPDATE steward_budget_reservations SET state='released' WHERE task_id=?1 AND state='reserved'",
            [task_id],
        )
        .map_err(database_error)?;
    Ok(())
}
