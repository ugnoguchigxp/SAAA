//! Tool-call ledger decorator: reserve -> dispatched -> settled, committed around every call so a
//! crash leaves an honest trace (`dispatched` means the call may have happened).
use super::store::db;
use crate::persistence::SqliteWriter;
use crate::worker_agents::contracts::*;
use crate::RunCancellation;
use async_trait::async_trait;
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

const NOT_ALLOWED: &str = r#"{"error":{"code":"tool_not_allowed"}}"#;
const LEDGER_UNAVAILABLE: &str = r#"{"error":{"code":"ledger_unavailable"}}"#;

pub(crate) struct LedgeredToolRunner<'a> {
    writer: &'a SqliteWriter,
    task_id: &'a str,
    attempt_ordinal: u32,
    tools: &'a [LoadedTool],
    inner: &'a dyn ToolRunner,
    step: AtomicU32,
}

impl<'a> LedgeredToolRunner<'a> {
    pub(crate) fn new(
        writer: &'a SqliteWriter,
        task_id: &'a str,
        attempt_ordinal: u32,
        tools: &'a [LoadedTool],
        inner: &'a dyn ToolRunner,
    ) -> Self {
        Self {
            writer,
            task_id,
            attempt_ordinal,
            tools,
            inner,
            step: AtomicU32::new(0),
        }
    }

    /// Number of calls attempted through this wrapper (listed tools only).
    pub(crate) fn steps(&self) -> u32 {
        self.step.load(Ordering::SeqCst)
    }

    fn set_state(
        &self,
        operation_key: &str,
        state: &str,
        outcome: Option<&str>,
    ) -> Result<(), String> {
        self.writer.transact(|connection| {
            connection
                .execute(
                    "UPDATE worker_tool_calls
                     SET dispatch_state = ?3, outcome = COALESCE(?4, outcome), updated_at_ms = ?5
                     WHERE task_id = ?1 AND operation_key = ?2",
                    params![self.task_id, operation_key, state, outcome, now_ms()],
                )
                .map(|_| ())
                .map_err(db)
        })
    }
}

fn now_ms() -> i64 {
    crate::worker_agents::loader::now_ms()
}

/// Short, content-free description of a tool result for the ledger.
fn outcome_summary(result: &str) -> String {
    let error_code = serde_json::from_str::<serde_json::Value>(result)
        .ok()
        .and_then(|value| {
            let error = value.get("error")?;
            Some(
                error
                    .get("code")
                    .and_then(|code| code.as_str())
                    .or_else(|| error.as_str())
                    .unwrap_or("error")
                    .chars()
                    .take(60)
                    .collect::<String>(),
            )
        });
    match error_code {
        Some(code) => format!("error:{code} bytes={}", result.len()),
        None => format!("ok bytes={}", result.len()),
    }
}

#[async_trait]
impl ToolRunner for LedgeredToolRunner<'_> {
    async fn run(
        &self,
        tool_key: &str,
        arguments_json: &str,
        timeout: Duration,
        cancellation: &RunCancellation,
    ) -> String {
        // A tool the profile did not list is never executed.
        let Some(tool) = self.tools.iter().find(|tool| tool.key == tool_key) else {
            return NOT_ALLOWED.to_string();
        };
        let step = self.step.fetch_add(1, Ordering::SeqCst) + 1;
        let digest = format!("{:x}", Sha256::digest(arguments_json.as_bytes()));
        let operation_key = format!(
            "{}:{}:{}:{}",
            self.attempt_ordinal,
            step,
            tool_key,
            &digest[..16]
        );
        let reserved = self.writer.transact(|connection| {
            let now = now_ms();
            connection
                .execute(
                    "INSERT INTO worker_tool_calls(task_id, operation_key, attempt_ordinal, tool_key,
                        effect, dispatch_state, outcome, created_at_ms, updated_at_ms)
                     VALUES(?1, ?2, ?3, ?4, ?5, 'reserved', NULL, ?6, ?6)",
                    params![
                        self.task_id,
                        operation_key,
                        self.attempt_ordinal,
                        tool_key,
                        tool.effect.as_str(),
                        now
                    ],
                )
                .map(|_| ())
                .map_err(db)
        });
        if reserved.is_err() || self.set_state(&operation_key, "dispatched", None).is_err() {
            return LEDGER_UNAVAILABLE.to_string();
        }
        // If this future is dropped here (deadline, cancel, crash) the row stays `dispatched`:
        // recovery treats that as an unknown outcome.
        let result = self
            .inner
            .run(tool_key, arguments_json, timeout, cancellation)
            .await;
        let _ = self.set_state(&operation_key, "settled", Some(&outcome_summary(&result)));
        result
    }
}
