//! Plan successor enqueue and bounded replan.
use super::repository as repo;
use crate::database_error;
use rusqlite::{params, Connection};

pub(crate) fn finish_goal_if_complete(
    connection: &Connection,
    goal_id: &str,
) -> Result<(), String> {
    let pending: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM steward_plan_steps s
             JOIN steward_goal_plans p ON p.id=s.plan_id
             WHERE p.goal_id=?1 AND p.revision=(SELECT MAX(revision) FROM steward_goal_plans WHERE goal_id=?1)
               AND NOT EXISTS (
                 SELECT 1 FROM steward_tasks t JOIN steward_delegations d ON d.id=t.delegation_id
                 WHERE d.goal_id=?1 AND t.goal_plan_id=p.id AND t.plan_step_id=s.step_id AND t.loop_state='done'
               )",
            [goal_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if pending == 0 {
        connection
            .execute(
                "UPDATE steward_goal_progress SET work_status='done',revision=revision+1,updated_at=?2,technical_state='complete',verified_success=1 WHERE goal_id=?1",
                params![goal_id, crate::now_iso()],
            )
            .map_err(database_error)?;
    }
    Ok(())
}

pub(crate) fn replan_after_failure(connection: &Connection, task_id: &str) -> Result<(), String> {
    repo::replan_after_failure(connection, task_id)?;
    Ok(())
}
