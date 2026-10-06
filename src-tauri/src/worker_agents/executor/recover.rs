//! Restart recovery and cancellation (docs/plans/worker-agents.md §5.5).
use super::delivery::{finalize, Terminal};
use super::executor::Executor;
use super::signals;
use super::store::{self, db, TaskRow, ACTIVE_STATES};
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::{load_revision, now_ms};
use rusqlite::{params, Connection};

impl Executor {
    /// Runs [`recover`] in one transaction and asks the runtime to flush any outbox rows it made.
    /// Call once at startup, after `task_queue::recover` has handled the worker lane's leases.
    pub(crate) fn recover(&self) -> Result<usize, String> {
        let conversations = self
            .writer
            .transact(|connection| recover(connection, now_ms()))?;
        self.notify_reports(&conversations);
        Ok(conversations.len())
    }
}

/// Settles every non-terminal worker task left by a previous process.
///
/// - The waiting conversation job died with the process, so every survivor is delivered
///   asynchronously from now on.
/// - A `running`/`verifying` task is re-queued once per `limits.max_restarts` only when all its
///   tools are read-only and no non-read-only call is `dispatched`/`unknown`; otherwise it
///   fails as `Interrupted`.
/// - Every surviving task is guaranteed a live queue job (a fresh generation if the old one was
///   interrupted), so the result does not depend on how the queue lane itself was recovered.
///
/// Idempotent. Returns the conversations that received an outbox row (flush them after commit).
pub(crate) fn recover(connection: &Connection, now_ms: i64) -> Result<Vec<String>, String> {
    let mut to_flush: Vec<String> = Vec::new();
    for task in store::active_tasks(connection)? {
        connection
            .execute(
                "UPDATE worker_tasks SET delivery='async_queued' WHERE id=?1 AND delivery='sync_waiting'",
                params![task.id],
            )
            .map_err(db)?;
        if task.state != "accepted" {
            let restartable = restartable(connection, &task)?;
            if restartable {
                connection
                    .execute(
                        "UPDATE worker_tasks SET state='accepted', restarts=restarts+1,
                            updated_at_ms=?2 WHERE id=?1 AND state IN ('running','verifying')",
                        params![task.id, now_ms],
                    )
                    .map_err(db)?;
            }
            connection
                .execute(
                    "UPDATE worker_attempts SET status='interrupted', finished_at_ms=?2
                     WHERE task_id=?1 AND status='running'",
                    params![task.id, now_ms],
                )
                .map_err(db)?;
            if !restartable {
                let done = finalize(
                    connection,
                    &task.id,
                    &Terminal::Failed(FailureCode::Interrupted),
                    now_ms,
                )?;
                if done.reported && !to_flush.contains(&done.conversation_id) {
                    to_flush.push(done.conversation_id);
                }
                continue;
            }
        }
        ensure_live_job(connection, &task)?;
    }
    Ok(to_flush)
}

fn restartable(connection: &Connection, task: &TaskRow) -> Result<bool, String> {
    let Ok(revision) = load_revision(connection, &task.profile_revision_id) else {
        return Ok(false);
    };
    Ok(revision.all_tools_read_only()
        && !store::has_unsettled_effect(connection, &task.id)?
        && task.restarts < i64::from(revision.limits.max_restarts))
}

fn ensure_live_job(connection: &Connection, task: &TaskRow) -> Result<(), String> {
    let (live, newest): (bool, Option<i64>) = connection
        .query_row(
            "SELECT COALESCE(MAX(state IN ('queued','running')), 0), MAX(generation)
             FROM task_queue_jobs WHERE scope=?1 AND kind=?2 AND job_key=?3",
            params![task.conversation_id, WORKER_JOB_KIND, task.id],
            |row| Ok((row.get::<_, i64>(0)? == 1, row.get(1)?)),
        )
        .map_err(db)?;
    if live {
        return Ok(());
    }
    crate::task_queue::enqueue(
        connection,
        &task.conversation_id,
        WORKER_LANE,
        WORKER_JOB_KIND,
        &task.id,
        newest.map_or(0, |generation| generation + 1),
        &serde_json::json!({ "taskId": task.id }).to_string(),
        None,
    )?;
    Ok(())
}

/// The queue gave up on a worker job after repeated infrastructure faults. Settles its task as
/// `Interrupted` (no-op when it is already terminal) so it neither stays active forever nor
/// re-runs at the next restart. An async task still reaches the user through the outbox.
pub(crate) fn abandon_job(connection: &Connection, job_payload: &str) -> Result<(), String> {
    let Some(task_id) = serde_json::from_str::<serde_json::Value>(job_payload)
        .ok()
        .and_then(|value| value.get("taskId")?.as_str().map(str::to_string))
    else {
        return Ok(());
    };
    finalize(
        connection,
        &task_id,
        &Terminal::Failed(FailureCode::Interrupted),
        now_ms(),
    )
    .map(|_| ())
}

/// Cancels one active task: `cancelled`, `delivery='suppressed'` (it never enters the outbox),
/// its queue job is cancelled and a running attempt is told to stop. Returns whether the task
/// was active.
pub(crate) fn cancel_task(connection: &Connection, task_id: &str) -> Result<bool, String> {
    let Some(task) = store::load_task(connection, task_id)? else {
        return Ok(false);
    };
    let changed = connection
        .execute(
            &format!(
                "UPDATE worker_tasks SET state='cancelled', failure_code='cancelled',
                    delivery='suppressed', updated_at_ms=?2
                 WHERE id=?1 AND state IN {ACTIVE_STATES}"
            ),
            params![task_id, now_ms()],
        )
        .map_err(db)?;
    if changed == 0 {
        return Ok(false);
    }
    crate::task_queue::cancel_key(connection, &task.conversation_id, task_id)?;
    signals::fire(task_id);
    Ok(true)
}

/// Cancels every active worker task started for one user input (the user cancelled that input).
/// Returns the ids of the tasks that were cancelled.
pub(crate) fn cancel_for_input(
    connection: &Connection,
    conversation_id: &str,
    input_message_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(&format!(
            "SELECT id FROM worker_tasks
             WHERE conversation_id=?1 AND input_message_id=?2 AND state IN {ACTIVE_STATES}"
        ))
        .map_err(db)?;
    let ids = statement
        .query_map(params![conversation_id, input_message_id], |row| {
            row.get::<_, String>(0)
        })
        .map_err(db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db)?;
    drop(statement);
    let mut cancelled = Vec::new();
    for id in ids {
        if cancel_task(connection, &id)? {
            cancelled.push(id);
        }
    }
    Ok(cancelled)
}
