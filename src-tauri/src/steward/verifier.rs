//! Step and Goal verifiers consume structured evidence, never model success prose.
use super::execution_contracts::VerifierOutcome;
use crate::database_error;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(crate) fn evaluate_task(
    connection: &Connection,
    task_id: &str,
) -> Result<VerifierOutcome, String> {
    let (verifier, recipe, job): (String, Option<String>, Option<String>) = connection
        .query_row(
            "SELECT COALESCE(s.verifier,g.verifier),s.recipe,t.coding_job_id
             FROM steward_tasks t
             JOIN steward_delegations d ON d.id=t.delegation_id
             JOIN steward_goals g ON g.id=d.goal_id
             LEFT JOIN steward_plan_steps s ON s.plan_id=t.goal_plan_id AND s.step_id=t.plan_step_id
             WHERE t.id=?1",
            [task_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(database_error)?;
    let evidence = job_evidence(connection, job.as_deref())?;
    let outcome = match verifier.as_str() {
        "test_report_obtained" => {
            if job.is_some() {
                VerifierOutcome::Pass
            } else {
                VerifierOutcome::Missing
            }
        }
        "tests_pass" => match evidence.get("exitCode").and_then(Value::as_i64) {
            Some(0) => VerifierOutcome::Pass,
            Some(_) => VerifierOutcome::Fail,
            None => VerifierOutcome::Missing,
        },
        "user_confirmation_required" => VerifierOutcome::AwaitingUser,
        _ if recipe.as_deref() == Some("read") => {
            if evidence.get("artifactDigest").is_some() || job.is_some() {
                VerifierOutcome::Pass
            } else {
                VerifierOutcome::Missing
            }
        }
        _ => VerifierOutcome::Unknown,
    };
    connection
        .execute(
            "INSERT INTO steward_verifier_outcomes(id,task_id,verifier,outcome,evidence_json,created_at)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                crate::new_id("verifier"),
                task_id,
                verifier,
                outcome_key(&outcome),
                evidence.to_string(),
                crate::now_iso()
            ],
        )
        .map_err(database_error)?;
    Ok(outcome)
}

fn job_evidence(connection: &Connection, job: Option<&str>) -> Result<Value, String> {
    let Some(job) = job else {
        return Ok(json!({}));
    };
    let row: Option<(Option<String>, String)> = connection
        .query_row(
            "SELECT r.result_json,r.state FROM coding_runs r WHERE r.job_id=?1 ORDER BY r.rowid DESC LIMIT 1",
            [job],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(database_error)?;
    let Some((result, state)) = row else {
        return Ok(json!({"jobId": job}));
    };
    let mut evidence = result
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({"jobId": job, "runState": state}));
    Ok(evidence)
}

pub(crate) fn outcome_key(outcome: &VerifierOutcome) -> &'static str {
    match outcome {
        VerifierOutcome::Pass => "pass",
        VerifierOutcome::Fail => "fail",
        VerifierOutcome::Missing => "missing",
        VerifierOutcome::AwaitingUser => "awaiting_user",
        VerifierOutcome::Unknown => "unknown",
    }
}

pub(crate) fn map_to_task_state(outcome: &VerifierOutcome) -> &'static str {
    match outcome {
        VerifierOutcome::Pass => "done",
        VerifierOutcome::Fail => "failed",
        VerifierOutcome::Missing | VerifierOutcome::AwaitingUser => "awaiting_user",
        VerifierOutcome::Unknown => "outcome_unknown",
    }
}
