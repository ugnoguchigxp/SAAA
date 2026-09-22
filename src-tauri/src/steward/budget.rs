//! Budget reservation, consumption, and remaining deadline.
use super::repository::ActiveWork;
use crate::database_error;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

fn armed_deadlines() -> &'static Mutex<HashMap<String, i64>> {
    static ARMED: LazyLock<Mutex<HashMap<String, i64>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    &ARMED
}

/// Hands a delegation's remaining milliseconds to the runner for `run_id`.
pub(crate) fn arm(run_id: &str, remaining_ms: i64) {
    if let Ok(mut armed) = armed_deadlines().lock() {
        armed.insert(run_id.to_string(), remaining_ms.max(1));
    }
}

pub(crate) fn take_armed(run_id: &str) -> Option<i64> {
    armed_deadlines().lock().ok()?.remove(run_id)
}

pub(crate) fn remaining_deadline_ms(
    connection: &Connection,
    work: &ActiveWork,
) -> Result<i64, String> {
    let now = crate::now_iso();
    let consumed: i64 = connection
        .query_row(
            "SELECT COALESCE(SUM(max(0,
                CAST(COALESCE(r.ended_at, ?2) AS INTEGER) - CAST(r.started_at AS INTEGER)
             )),0)
             FROM coding_runs r
             JOIN coding_jobs j ON j.id=r.job_id
             JOIN steward_tasks t ON t.coding_job_id=j.id
             WHERE t.delegation_id=?1",
            params![work.delegation_id, now],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    Ok((work.budget_ms - consumed).max(1))
}

/// Remaining wait for a steward-linked run. `None` means the run is not delegated.
pub(crate) fn remaining_for_run(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<i64>, String> {
    let work = connection
        .query_row(
            "SELECT g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,0,d.ops,g.verifier
             FROM coding_runs r
             JOIN steward_tasks t ON t.coding_job_id=r.job_id
             JOIN steward_delegations d ON d.id=t.delegation_id
             JOIN steward_goals g ON g.id=d.goal_id
             WHERE r.id=?1",
            [run_id],
            |row| {
                Ok(ActiveWork {
                    goal_id: row.get(0)?,
                    goal_status: row.get(1)?,
                    delegation_id: row.get(2)?,
                    workspace_id: row.get(3)?,
                    budget_runs: row.get(4)?,
                    budget_ms: row.get(5)?,
                    superseded: row.get::<_, i64>(6)? != 0,
                    ops: row.get(7)?,
                    verifier: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(database_error)?;
    match work {
        Some(work) => Ok(Some(remaining_deadline_ms(connection, &work)?)),
        None => Ok(None),
    }
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
