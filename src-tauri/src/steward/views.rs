//! Read-only Goal snapshots. Queries never dispatch or publish.
use super::execution_contracts::{GoalProgress, StewardGoalView};
use crate::database_error;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub(crate) fn list_goals(connection: &Connection, conversation_id: &str) -> Result<Value, String> {
    let mut statement = connection
        .prepare(
            "SELECT g.id,g.summary,g.status,COALESCE(p.work_status,'queued'),d.workspace_id,d.ops,g.verifier,d.budget_runs,d.budget_ms,d.notify,
                    (SELECT t.last_error FROM steward_tasks t JOIN steward_delegations x ON x.id=t.delegation_id WHERE x.goal_id=g.id AND t.last_error IS NOT NULL ORDER BY t.rowid DESC LIMIT 1),
                    COALESCE(p.revision,g.revision)
             FROM steward_goals g
             JOIN steward_delegations d ON d.goal_id=g.id AND d.superseded_by IS NULL
             LEFT JOIN steward_goal_progress p ON p.goal_id=g.id
             WHERE g.conversation_id=?1
             ORDER BY g.rowid",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([conversation_id], |row| {
            Ok(StewardGoalView {
                goal_id: row.get(0)?,
                summary: row.get(1)?,
                authority_status: row.get(2)?,
                progress: parse_progress(&row.get::<_, String>(3)?),
                workspace_id: row.get(4)?,
                operations: row.get(5)?,
                verifier: row.get(6)?,
                budget_runs: row.get(7)?,
                budget_ms: row.get(8)?,
                notify: row.get(9)?,
                awaiting_reason: row.get(10)?,
                report_revision: row.get(11)?,
                unsupported_profile: false,
            })
        })
        .map_err(database_error)?;
    let views: Vec<StewardGoalView> = rows.filter_map(Result::ok).collect();
    Ok(json!({"goals": views, "revision": views.len() as i64}))
}

fn parse_progress(value: &str) -> GoalProgress {
    match value {
        "running" => GoalProgress::Running,
        "awaiting_user" => GoalProgress::AwaitingUser,
        "done" => GoalProgress::Done,
        "failed" => GoalProgress::Failed,
        "cancelled" => GoalProgress::Cancelled,
        _ => GoalProgress::Queued,
    }
}

pub(crate) fn cursor(connection: &Connection, conversation_id: &str) -> Result<i64, String> {
    connection
        .query_row(
            "SELECT COALESCE((SELECT revision FROM steward_delivery_cursor WHERE conversation_id=?1),0)",
            params![conversation_id],
            |row| row.get(0),
        )
        .map_err(database_error)
}
