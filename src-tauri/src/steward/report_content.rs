//! Structured report bodies. Task ids alone are not a completion report.
use super::repository::TerminalReport;
use rusqlite::Connection;
use serde_json::json;

pub(crate) fn compose(
    connection: &Connection,
    terminal: &TerminalReport,
    loop_state: &str,
) -> String {
    let summary: String = connection
        .query_row(
            "SELECT summary FROM steward_goals WHERE id=?1",
            [&terminal.goal_id],
            |row| row.get(0),
        )
        .unwrap_or_default();
    let evidence: String = connection
        .query_row(
            "SELECT COALESCE(evidence_json,'{}') FROM steward_verifier_outcomes WHERE task_id=?1 ORDER BY rowid DESC LIMIT 1",
            [&terminal.task_id],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "{}".into());
    json!({
        "goalId": terminal.goal_id,
        "summary": summary,
        "taskId": terminal.task_id,
        "state": loop_state,
        "evidence": serde_json::from_str::<serde_json::Value>(&evidence).unwrap_or(json!({})),
        "nextAction": match loop_state {
            "awaiting_user" => "user_confirmation_required",
            "outcome_unknown" => "inspect_existing_run",
            "failed" => "review_failures",
            _ => "none",
        }
    })
    .to_string()
}
