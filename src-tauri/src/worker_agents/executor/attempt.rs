//! Tier ladder and retry policy (docs/plans/worker-agents.md §6.2).
//!
//! Same tier: retry `InvalidOutput`/`Transport`/`CompletionUnmet` up to
//! `limits.max_same_tier_retries`, then climb. `Terminal`, `Cancelled` and `DeadlineExceeded`
//! never climb. A cloud rung is never executed in v1: it ends the task with a recorded proposal.
use super::delivery::Terminal;
use super::executor::Executor;
use super::ledger::LedgeredToolRunner;
use super::store::{self, db, TaskRow};
use super::verify::verify_output;
use crate::worker_agents::contracts::*;
use crate::worker_agents::loader::now_ms;
use crate::RunCancellation;
use rusqlite::params;

/// A proposal that nobody answers in v1 still expires, so it cannot pile up as live.
const ESCALATION_TTL_MS: i64 = 10 * 60 * 1000;

fn is_cloud(route: &TierRoute) -> bool {
    route.tier == Tier::Cloud || route.location == RouteLocation::Cloud
}

fn error_name(error: &AttemptError) -> String {
    match error {
        AttemptError::InvalidOutput(_) => "invalid_output".into(),
        AttemptError::Transport(_) => "transport".into(),
        AttemptError::CompletionUnmet => "completion_unmet".into(),
        AttemptError::Terminal(code) => code.as_str().into(),
        AttemptError::Cancelled => "cancelled".into(),
        AttemptError::DeadlineExceeded => "deadline_exceeded".into(),
    }
}

/// The failure reported when every rung has been tried.
fn exhausted_code(error: &AttemptError) -> FailureCode {
    match error {
        AttemptError::InvalidOutput(_) => FailureCode::InvalidOutput,
        AttemptError::CompletionUnmet => FailureCode::CompletionUnmet,
        _ => FailureCode::ToolUnavailable,
    }
}

impl Executor {
    /// Rungs this revision may use, cheapest first. A cloud rung stays only when the profile
    /// allows proposing it; anything above `max_tier` is dropped.
    pub(super) fn ladder(&self, revision: &LoadedRevision) -> Result<Vec<TierRoute>, String> {
        let policy = &revision.tier_policy;
        Ok(self
            .model
            .tier_routes()?
            .into_iter()
            .filter(|route| route.tier <= policy.max_tier)
            .filter(|route| !is_cloud(route) || policy.cloud == CloudPolicy::RequireApproval)
            .collect())
    }

    pub(super) async fn run_ladder(
        &self,
        task: &TaskRow,
        revision: &LoadedRevision,
        input: &serde_json::Value,
        user_text: &str,
        ladder: &[TierRoute],
        cancellation: &RunCancellation,
    ) -> Result<Terminal, String> {
        let limits = &revision.limits;
        let start = usize::try_from(task.tier_index)
            .unwrap_or(0)
            .min(ladder.len() - 1);
        let mut last_error = AttemptError::Transport("no attempt was made".into());
        for (index, route) in ladder.iter().enumerate().skip(start) {
            if is_cloud(route) {
                self.propose_escalation(&task.id, route)?;
                return Ok(Terminal::Failed(FailureCode::EscalationRequiresApproval));
            }
            for _ in 0..=limits.max_same_tier_retries {
                match self
                    .attempt_once(task, revision, input, user_text, index, route, cancellation)
                    .await?
                {
                    Ok(output) => return Ok(Terminal::Succeeded(output)),
                    Err(AttemptError::Cancelled) => return Ok(Terminal::Cancelled),
                    Err(AttemptError::DeadlineExceeded) => {
                        return Ok(Terminal::Failed(FailureCode::DeadlineExceeded))
                    }
                    Err(AttemptError::Terminal(code)) => return Ok(Terminal::Failed(code)),
                    Err(error) => {
                        // A write/unknown effect that may have happened is never replayed.
                        let unsettled = self
                            .writer
                            .read_serialized(|c| store::has_unsettled_effect(c, &task.id))?;
                        if unsettled {
                            return Ok(Terminal::Failed(FailureCode::OutcomeUnknown));
                        }
                        last_error = error;
                    }
                }
            }
        }
        Ok(Terminal::Failed(exhausted_code(&last_error)))
    }

    /// One recorded attempt. The attempt row is committed before the runner starts. The outer
    /// `Result` is an infrastructure fault; the inner one is the attempt's own outcome.
    #[allow(clippy::too_many_arguments)]
    async fn attempt_once(
        &self,
        task: &TaskRow,
        revision: &LoadedRevision,
        input: &serde_json::Value,
        user_text: &str,
        index: usize,
        route: &TierRoute,
        cancellation: &RunCancellation,
    ) -> Result<Result<WorkerOutput, AttemptError>, String> {
        if cancellation.is_cancelled() {
            return Ok(Err(AttemptError::Cancelled));
        }
        let task_remaining = (task.deadline_at_ms - now_ms()).max(0) as u64;
        let budget =
            std::time::Duration::from_millis(task_remaining.min(revision.limits.deadline_ms));
        if budget.is_zero() {
            return Ok(Err(AttemptError::DeadlineExceeded));
        }
        let deadline = tokio::time::Instant::now() + budget;
        let ordinal = self.begin_attempt(&task.id, index, route)?;
        let ledgered = LedgeredToolRunner::new(
            &self.writer,
            &task.id,
            ordinal,
            &revision.tools,
            self.tools.as_ref(),
        );
        let env = AttemptEnv {
            task_id: &task.id,
            attempt_ordinal: ordinal,
            revision,
            input,
            user_text,
            writer: &self.writer,
            model: self.model.as_ref(),
            route,
            tools: &ledgered,
            cancellation,
            deadline,
        };
        let runner = match revision.output_kind {
            OutputKind::WebClaimsV1 => &self.runners.web_claims,
            OutputKind::JsonV1 => &self.runners.json,
        };
        let raced = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Err(AttemptError::Cancelled),
            run = tokio::time::timeout_at(deadline, runner.run(&env)) => {
                run.unwrap_or(Err(AttemptError::DeadlineExceeded))
            }
        };
        let steps = ledgered.steps();
        let result = match raced {
            Ok(output) => {
                self.set_task_state(&task.id, "verifying")?;
                let met = self
                    .writer
                    .read_serialized(|c| verify_output(c, &task.id, revision, &output))?;
                if met {
                    Ok(output)
                } else {
                    Err(AttemptError::CompletionUnmet)
                }
            }
            Err(error) => Err(error),
        };
        self.end_attempt(&task.id, ordinal, &result, steps)?;
        Ok(result)
    }

    fn set_task_state(&self, task_id: &str, state: &str) -> Result<(), String> {
        self.writer.transact(|connection| {
            connection
                .execute(
                    "UPDATE worker_tasks SET state=?2, updated_at_ms=?3
                     WHERE id=?1 AND state IN ('running','verifying')",
                    params![task_id, state, now_ms()],
                )
                .map(|_| ())
                .map_err(db)
        })
    }

    fn begin_attempt(&self, task_id: &str, index: usize, route: &TierRoute) -> Result<u32, String> {
        self.writer.transact(|connection| {
            let ordinal: u32 = connection
                .query_row(
                    "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM worker_attempts WHERE task_id=?1",
                    params![task_id],
                    |row| row.get(0),
                )
                .map_err(db)?;
            let now = now_ms();
            connection
                .execute(
                    "INSERT INTO worker_attempts(task_id, ordinal, tier, route_fingerprint, status,
                        steps_used, started_at_ms)
                     VALUES(?1, ?2, ?3, ?4, 'running', 0, ?5)",
                    params![
                        task_id,
                        ordinal,
                        route.tier.as_str(),
                        route.fingerprint,
                        now
                    ],
                )
                .map_err(db)?;
            connection
                .execute(
                    "UPDATE worker_tasks SET state='running', tier_index=?2, attempts=attempts+1,
                        updated_at_ms=?3
                     WHERE id=?1 AND state IN ('running','verifying')",
                    params![task_id, index as i64, now],
                )
                .map_err(db)?;
            Ok(ordinal)
        })
    }

    fn end_attempt(
        &self,
        task_id: &str,
        ordinal: u32,
        result: &Result<WorkerOutput, AttemptError>,
        steps: u32,
    ) -> Result<(), String> {
        let (status, code) = match result {
            Ok(_) => ("succeeded", None),
            Err(AttemptError::Cancelled) => ("cancelled", Some("cancelled".to_string())),
            Err(error) => ("failed", Some(error_name(error))),
        };
        self.writer.transact(|connection| {
            connection
                .execute(
                    "UPDATE worker_attempts SET status=?3, error_code=?4, steps_used=?5,
                        finished_at_ms=?6
                     WHERE task_id=?1 AND ordinal=?2",
                    params![task_id, ordinal, status, code, steps, now_ms()],
                )
                .map(|_| ())
                .map_err(db)
        })
    }

    /// Records the cloud rung as a proposal. v1 has no approval flow, so nothing consumes it and
    /// the model is never called on that rung.
    fn propose_escalation(&self, task_id: &str, route: &TierRoute) -> Result<(), String> {
        self.writer.transact(|connection| {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM worker_escalations
                      WHERE task_id=?1 AND tier=?2 AND status='proposed')",
                    params![task_id, route.tier.as_str()],
                    |row| row.get(0),
                )
                .map_err(db)?;
            if exists {
                return Ok(());
            }
            let now = now_ms();
            connection
                .execute(
                    "INSERT INTO worker_escalations(id, task_id, tier, route_fingerprint,
                        estimated_cost_micros, status, expires_at_ms, created_at_ms)
                     VALUES(?1, ?2, ?3, ?4, NULL, 'proposed', ?5, ?6)",
                    params![
                        crate::new_id("wesc"),
                        task_id,
                        route.tier.as_str(),
                        route.fingerprint,
                        now + ESCALATION_TTL_MS,
                        now
                    ],
                )
                .map(|_| ())
                .map_err(db)
        })
    }
}
