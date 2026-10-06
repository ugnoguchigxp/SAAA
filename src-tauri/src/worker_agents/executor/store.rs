//! Task-row reads and the outcome mapping shared by admission, execution and recovery.
use crate::worker_agents::contracts::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// SQL list of the non-terminal task states.
pub(super) const ACTIVE_STATES: &str = "('accepted','running','verifying')";

pub(super) fn db(error: rusqlite::Error) -> String {
    crate::database_error(error)
}

pub(super) fn is_active(state: &str) -> bool {
    matches!(state, "accepted" | "running" | "verifying")
}

#[derive(Debug, Clone)]
pub(super) struct TaskRow {
    pub id: String,
    pub conversation_id: String,
    pub input_message_id: String,
    pub profile_revision_id: String,
    pub input_json: String,
    pub state: String,
    pub delivery: String,
    pub tier_index: i64,
    pub restarts: i64,
    pub deadline_at_ms: i64,
    pub result_json: Option<String>,
    pub failure_code: Option<String>,
}

pub(super) fn load_task(connection: &Connection, task_id: &str) -> Result<Option<TaskRow>, String> {
    connection
        .query_row(
            "SELECT id, conversation_id, input_message_id, profile_revision_id, input_json, state,
                    delivery, tier_index, restarts, deadline_at_ms, result_json, failure_code
             FROM worker_tasks WHERE id = ?1",
            params![task_id],
            task_from_row,
        )
        .optional()
        .map_err(db)
}

pub(super) fn active_tasks(connection: &Connection) -> Result<Vec<TaskRow>, String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT id, conversation_id, input_message_id, profile_revision_id, input_json, state,
                    delivery, tier_index, restarts, deadline_at_ms, result_json, failure_code
             FROM worker_tasks WHERE state IN {ACTIVE_STATES} ORDER BY created_at_ms, id"
        ))
        .map_err(db)?;
    let rows = statement.query_map([], task_from_row).map_err(db)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(db)
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRow> {
    Ok(TaskRow {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        input_message_id: row.get(2)?,
        profile_revision_id: row.get(3)?,
        input_json: row.get(4)?,
        state: row.get(5)?,
        delivery: row.get(6)?,
        tier_index: row.get(7)?,
        restarts: row.get(8)?,
        deadline_at_ms: row.get(9)?,
        result_json: row.get(10)?,
        failure_code: row.get(11)?,
    })
}

/// Failures a caller may reasonably try again later.
pub(super) fn retryable(code: FailureCode) -> bool {
    matches!(
        code,
        FailureCode::DeadlineExceeded | FailureCode::ToolUnavailable | FailureCode::Interrupted
    )
}

pub(super) fn failed(task_id: Option<&str>, code: FailureCode) -> WorkerOutcome {
    WorkerOutcome::Failed {
        task_id: task_id.map(str::to_string),
        failure: WorkerFailure {
            code,
            retryable: retryable(code),
        },
    }
}

/// The outcome recorded on a terminal row. Non-terminal rows are `Pending`.
pub(super) fn outcome_of(row: &TaskRow) -> WorkerOutcome {
    let task_id = Some(row.id.as_str());
    match row.state.as_str() {
        "succeeded" => row
            .result_json
            .as_deref()
            .and_then(|text| serde_json::from_str::<WorkerOutput>(text).ok())
            .map(|output| WorkerOutcome::Succeeded {
                task_id: row.id.clone(),
                output,
            })
            .unwrap_or_else(|| failed(task_id, FailureCode::InvalidOutput)),
        "failed" => failed(
            task_id,
            row.failure_code
                .as_deref()
                .and_then(FailureCode::parse)
                .unwrap_or(FailureCode::OutcomeUnknown),
        ),
        "cancelled" => failed(task_id, FailureCode::Cancelled),
        _ => WorkerOutcome::Pending {
            task_id: row.id.clone(),
        },
    }
}

/// What the waiting conversation job is told about `task_id`, or `None` while the task is still
/// running. A terminal read hands the result to the caller exactly once: it flips
/// `sync_waiting` to `sync_delivered`. A task already routed to the outbox answers `Pending`, so a
/// replayed conversation job cannot present the same result a second time.
pub(super) fn deliver_sync(
    connection: &Connection,
    task_id: &str,
) -> Result<Option<WorkerOutcome>, String> {
    let Some(row) = load_task(connection, task_id)? else {
        return Ok(Some(failed(None, FailureCode::NoMatchingAgent)));
    };
    if is_active(&row.state) {
        return Ok(None);
    }
    match row.delivery.as_str() {
        "sync_waiting" => {
            connection
                .execute(
                    "UPDATE worker_tasks SET delivery='sync_delivered' WHERE id=?1 AND delivery='sync_waiting'",
                    params![task_id],
                )
                .map_err(db)?;
            Ok(Some(outcome_of(&row)))
        }
        "async_queued" | "async_delivered" => Ok(Some(WorkerOutcome::Pending {
            task_id: row.id.clone(),
        })),
        _ => Ok(Some(outcome_of(&row))),
    }
}

/// Recursively key-sorted JSON text, so the idempotency key does not depend on key order.
pub(super) fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_default(),
                        canonical_json(&map[key])
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

/// True when a tool call with an external effect may have happened without being settled.
pub(super) fn has_unsettled_effect(connection: &Connection, task_id: &str) -> Result<bool, String> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM worker_tool_calls
              WHERE task_id=?1 AND dispatch_state IN ('dispatched','unknown')
                AND effect NOT IN ('pure','read'))",
            params![task_id],
            |row| row.get(0),
        )
        .map_err(db)
}
