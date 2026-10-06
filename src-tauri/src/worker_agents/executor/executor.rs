//! The worker-lane executor: owns the seams (model, tools, per-kind runners) and drives one task
//! from `accepted` to a recorded terminal outcome.
use super::delivery::{finalize, Finalized, Terminal};
use super::store::{self, db, ACTIVE_STATES};
use super::{signals, ReportHook, Runners};
use crate::persistence::SqliteWriter;
use crate::task_queue::Job;
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::{load_revision, now_ms};
use crate::RunCancellation;
use rusqlite::params;
use std::sync::Arc;
use std::time::Duration;

pub(crate) struct Executor {
    pub(super) writer: Arc<SqliteWriter>,
    pub(super) model: Arc<dyn WorkerModel>,
    pub(super) tools: Arc<dyn ToolRunner>,
    pub(super) runners: Runners,
    pub(super) report_hook: Option<ReportHook>,
}

impl Executor {
    pub(crate) fn new(
        writer: Arc<SqliteWriter>,
        model: Arc<dyn WorkerModel>,
        tools: Arc<dyn ToolRunner>,
        runners: Runners,
    ) -> Self {
        Self {
            writer,
            model,
            tools,
            runners,
            report_hook: None,
        }
    }

    /// Called with a conversation id after a terminal/recovery transaction committed a steward
    /// outbox row for it. The runtime layer flushes the outbox there (it needs app state).
    pub(crate) fn with_report_hook(mut self, hook: ReportHook) -> Self {
        self.report_hook = Some(hook);
        self
    }

    pub(super) fn notify_reports(&self, conversation_ids: &[String]) {
        if let Some(hook) = &self.report_hook {
            for conversation_id in conversation_ids {
                hook(conversation_id);
            }
        }
    }

    /// Commits the terminal state of `task_id`, wakes any waiter and requests an outbox flush.
    pub(super) fn finish(&self, task_id: &str, terminal: &Terminal) -> Result<Finalized, String> {
        let finalized = self
            .writer
            .transact(|connection| finalize(connection, task_id, terminal, now_ms()))?;
        signals::wake(task_id);
        if finalized.reported {
            self.notify_reports(std::slice::from_ref(&finalized.conversation_id));
        }
        Ok(finalized)
    }

    /// Runs one worker-lane job. A failed task is a recorded outcome, not a lane error: `Err` is
    /// returned only for infrastructure faults, so the queue can retry the job.
    pub(crate) async fn process(
        &self,
        job: &Job,
        cancellation: &RunCancellation,
    ) -> Result<(), String> {
        let task_id = serde_json::from_str::<serde_json::Value>(&job.payload)
            .ok()
            .and_then(|value| value.get("taskId")?.as_str().map(str::to_string))
            .unwrap_or_else(|| job.key.clone());
        // A second job for a task already running in this process must not run it twice.
        let Some(_registration) = signals::register(&task_id, cancellation) else {
            return Ok(());
        };
        self.drive(&task_id, cancellation).await
    }

    async fn drive(&self, task_id: &str, cancellation: &RunCancellation) -> Result<(), String> {
        // Claim: only accepted/running tasks run; a cancel that landed first wins.
        let claimed = self.writer.transact(|connection| {
            let Some(task) = store::load_task(connection, task_id)? else {
                return Ok(None);
            };
            let changed = connection
                .execute(
                    "UPDATE worker_tasks SET state='running', updated_at_ms=?2
                     WHERE id=?1 AND state IN ('accepted','running','verifying')",
                    params![task_id, now_ms()],
                )
                .map_err(db)?;
            Ok((changed == 1).then_some(task))
        })?;
        let Some(task) = claimed else {
            return Ok(());
        };
        if cancellation.is_cancelled() {
            self.finish(task_id, &Terminal::Cancelled)?;
            return Ok(());
        }
        if now_ms() >= task.deadline_at_ms {
            self.finish(task_id, &Terminal::Failed(FailureCode::DeadlineExceeded))?;
            return Ok(());
        }
        let loaded = self.writer.read_serialized(|connection| {
            let revision = load_revision(connection, &task.profile_revision_id).ok();
            let user_text: String = connection
                .query_row(
                    "SELECT content FROM conversation_messages WHERE id=?1",
                    params![task.input_message_id],
                    |row| row.get(0),
                )
                .unwrap_or_default();
            Ok((revision, user_text))
        })?;
        let (Some(revision), user_text) = loaded else {
            self.finish(task_id, &Terminal::Failed(FailureCode::ToolUnavailable))?;
            return Ok(());
        };
        let Ok(input) = serde_json::from_str::<serde_json::Value>(&task.input_json) else {
            self.finish(task_id, &Terminal::Failed(FailureCode::InputInvalid))?;
            return Ok(());
        };
        let ladder = match self.ladder(&revision) {
            Ok(ladder) if !ladder.is_empty() => ladder,
            _ => {
                self.finish(task_id, &Terminal::Failed(FailureCode::ToolUnavailable))?;
                return Ok(());
            }
        };
        let terminal = self
            .run_ladder(&task, &revision, &input, &user_text, &ladder, cancellation)
            .await?;
        self.finish(task_id, &terminal)?;
        Ok(())
    }

    /// Terminal outcome for a caller that wants to wait for `task_id` for at most `wait`.
    ///
    /// Always re-reads the database; the in-process notify only shortens the wait. A task still
    /// running when the wait ends is switched to `async_queued` atomically (so its terminal
    /// transaction reports through the outbox) and `Pending` is returned.
    pub(crate) async fn wait_terminal(&self, task_id: &str, wait: Duration) -> WorkerOutcome {
        let until = tokio::time::Instant::now() + wait;
        let unknown = || store::failed(Some(task_id), FailureCode::OutcomeUnknown);
        // Taken before the first database read: a wake between that read and the wait is kept.
        let notify = signals::notifier(task_id);
        loop {
            match self
                .writer
                .transact(|connection| store::deliver_sync(connection, task_id))
            {
                Ok(Some(outcome)) => {
                    signals::forget(task_id);
                    return outcome;
                }
                Ok(None) => {}
                Err(_) => return unknown(),
            }
            let remaining = until.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let _ = tokio::time::timeout(remaining, notify.notified()).await;
        }
        let settled = self.writer.transact(|connection| {
            let changed = connection
                .execute(
                    &format!(
                        "UPDATE worker_tasks SET delivery='async_queued', updated_at_ms=?2
                         WHERE id=?1 AND state IN {ACTIVE_STATES} AND delivery='sync_waiting'"
                    ),
                    params![task_id, now_ms()],
                )
                .map_err(db)?;
            if changed == 1 {
                return Ok(Some(WorkerOutcome::Pending {
                    task_id: task_id.to_string(),
                }));
            }
            // Either already async (still running) or it became terminal while we gave up.
            store::deliver_sync(connection, task_id).map(|outcome| {
                Some(outcome.unwrap_or(WorkerOutcome::Pending {
                    task_id: task_id.to_string(),
                }))
            })
        });
        signals::forget(task_id);
        match settled {
            Ok(Some(outcome)) => outcome,
            _ => unknown(),
        }
    }
}
