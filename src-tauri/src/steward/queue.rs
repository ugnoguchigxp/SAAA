//! Fair, cross-conversation selection of dispatchable steward work.
use super::repository::ActiveWork;
use crate::database_error;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

pub(crate) fn next_eligible(
    connection: &Connection,
    conversation_id: Option<&str>,
) -> Result<Option<(ActiveWork, String, String)>, String> {
    let now = now_ms();
    let sql = if conversation_id.is_some() {
        "SELECT g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,0,d.ops,g.verifier,t.id,t.conversation_id
         FROM steward_tasks t
         JOIN steward_delegations d ON d.id=t.delegation_id
         JOIN steward_goals g ON g.id=d.goal_id
         JOIN steward_dispatch_intents i ON i.task_id=t.id
         WHERE t.conversation_id=?1 AND t.loop_state='queued' AND i.state='pending'
           AND g.status='active' AND g.superseded_by IS NULL
           AND d.status='active' AND d.superseded_by IS NULL
           AND (i.next_eligible_at IS NULL OR i.next_eligible_at<=?2)
         ORDER BY t.queue_rank, t.rowid LIMIT 1"
    } else {
        "SELECT g.id,g.status,d.id,d.workspace_id,d.budget_runs,d.budget_ms,0,d.ops,g.verifier,t.id,t.conversation_id
         FROM steward_tasks t
         JOIN steward_delegations d ON d.id=t.delegation_id
         JOIN steward_goals g ON g.id=d.goal_id
         JOIN steward_dispatch_intents i ON i.task_id=t.id
         WHERE t.loop_state='queued' AND i.state='pending'
           AND g.status='active' AND g.superseded_by IS NULL
           AND d.status='active' AND d.superseded_by IS NULL
           AND (i.next_eligible_at IS NULL OR i.next_eligible_at<=?1)
         ORDER BY t.queue_rank, t.rowid LIMIT 1"
    };
    let row = if let Some(conversation_id) = conversation_id {
        connection
            .query_row(sql, params![conversation_id, now], map_row)
            .optional()
    } else {
        connection.query_row(sql, [now], map_row).optional()
    };
    row.map_err(database_error)
}

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(ActiveWork, String, String)> {
    Ok((
        ActiveWork {
            goal_id: r.get(0)?,
            goal_status: r.get(1)?,
            delegation_id: r.get(2)?,
            workspace_id: r.get(3)?,
            budget_runs: r.get(4)?,
            budget_ms: r.get(5)?,
            superseded: r.get::<_, i64>(6)? != 0,
            ops: r.get(7)?,
            verifier: r.get(8)?,
        },
        r.get(9)?,
        r.get(10)?,
    ))
}

pub(crate) fn park_pending(
    connection: &Connection,
    task_id: &str,
    reason: &str,
) -> Result<(), String> {
    let attempt: i64 = connection
        .query_row(
            "SELECT attempt FROM steward_dispatch_intents WHERE task_id=?1",
            [task_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?
        .unwrap_or(0);
    let delay = 500i64.saturating_mul(1i64 << attempt.min(6));
    connection
        .execute(
            "UPDATE steward_dispatch_intents SET state='pending',last_reason=?2,attempt=attempt+1,next_eligible_at=?3,updated_at=?4 WHERE task_id=?1",
            params![task_id, reason, now_ms() + delay, crate::now_iso()],
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn idempotent_accepted(
    connection: &Connection,
    task_id: &str,
) -> Result<Option<Value>, String> {
    let row: Option<(String, Option<String>)> = connection
        .query_row(
            "SELECT state,receipt_json FROM steward_dispatch_intents WHERE task_id=?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(database_error)?;
    match row {
        Some((state, Some(receipt))) if state == "accepted" => serde_json::from_str(&receipt)
            .map(Some)
            .map_err(|_| "receipt_invalid".into()),
        Some((state, _)) if state == "outcome_unknown" => Err("outcome_unknown".into()),
        _ => Ok(None),
    }
}

pub(crate) fn recipe_for_task(
    connection: &Connection,
    work: &ActiveWork,
    task_id: &str,
) -> Result<&'static str, String> {
    Ok(
        match super::repository::task_step_recipe(connection, task_id)?.as_deref() {
            Some("read") => "read",
            Some("test_run") => "test_run",
            _ => match work.ops.as_str() {
                "read" => "read",
                "test_run" => "test_run",
                _ => "read_test",
            },
        },
    )
}

pub(crate) fn request_for_task(
    connection: &Connection,
    work: &ActiveWork,
    task_id: &str,
) -> Result<String, String> {
    let recipe = recipe_for_task(connection, work, task_id)?;
    let summary: String = connection
        .query_row(
            "SELECT summary FROM steward_goals WHERE id=?1",
            [&work.goal_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    let step = super::repository::task_step_recipe(connection, task_id)?
        .unwrap_or_else(|| recipe.to_string());
    let constraint = match recipe {
        "read" => {
            "Inspect the existing failure evidence in this workspace and report causes. Do not run tests or change files."
        }
        "test_run" => {
            "Run the relevant existing tests in this workspace and report their result. Do not change files."
        }
        _ => {
            "Inspect failing tests in this workspace. Read logs, run the relevant existing tests, and report causes. Do not change files."
        }
    };
    Ok(format!("Goal: {summary}\nRecipe: {step}\n{constraint}"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}
