//! Step and Goal verifiers consume host evidence, never model success prose.
use super::evidence::{self, EvidenceStatus, ExecutionEvidence, PRODUCER_LEGACY};
use super::execution_contracts::VerifierOutcome;
use crate::database_error;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub(crate) fn evaluate_task(
    connection: &Connection,
    task_id: &str,
    expected_run_id: Option<&str>,
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
    let outcome = evaluate_bound(
        connection,
        task_id,
        &verifier,
        recipe.as_deref(),
        job.as_deref(),
        expected_run_id,
    )?;
    persist_outcome(connection, task_id, expected_run_id, &verifier, &outcome)?;
    Ok(outcome.outcome)
}

pub(crate) struct Evaluation {
    pub outcome: VerifierOutcome,
    pub reason: String,
    pub evidence: Value,
}

fn evaluate_bound(
    connection: &Connection,
    task_id: &str,
    verifier: &str,
    recipe: Option<&str>,
    job: Option<&str>,
    expected_run_id: Option<&str>,
) -> Result<Evaluation, String> {
    if verifier == "user_confirmation_required" {
        return Ok(Evaluation {
            outcome: VerifierOutcome::AwaitingUser,
            reason: "user_confirmation_required".into(),
            evidence: json!({}),
        });
    }
    let Some(run_id) = expected_run_id else {
        return Ok(missing(
            "run_unbound",
            json!({"taskId": task_id, "jobId": job}),
        ));
    };
    let Some(job_id) = job else {
        return Ok(missing(
            "job_unbound",
            json!({"taskId": task_id, "runId": run_id}),
        ));
    };
    let adopted: Option<String> = connection
        .query_row(
            "SELECT current_run_id FROM coding_jobs WHERE id=?1",
            [job_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    if adopted.as_deref() != Some(run_id) {
        return Ok(unknown(
            "run_not_adopted",
            json!({"taskId": task_id, "jobId": job_id, "runId": run_id, "adoptedRun": adopted}),
        ));
    }
    let Some(evidence) = evidence::load_for_run(connection, run_id)? else {
        return Ok(missing(
            "evidence_missing",
            json!({"taskId": task_id, "jobId": job_id, "runId": run_id}),
        ));
    };
    if evidence.task_id != task_id || evidence.job_id != job_id {
        return Ok(unknown(
            "evidence_identity_mismatch",
            json!({"taskId": task_id, "jobId": job_id, "runId": run_id}),
        ));
    }
    evaluate_evidence(verifier, recipe, &evidence)
}

pub(crate) fn evaluate_evidence(
    verifier: &str,
    recipe: Option<&str>,
    evidence: &ExecutionEvidence,
) -> Result<Evaluation, String> {
    let payload = serde_json::to_value(evidence).unwrap_or(json!({}));
    match evidence::validate(evidence) {
        EvidenceStatus::Missing(reason) => return Ok(missing(reason, payload)),
        EvidenceStatus::Unknown(reason) => return Ok(unknown(reason, payload)),
        EvidenceStatus::Valid => {}
    }
    if evidence.producer == PRODUCER_LEGACY {
        return Ok(unknown("legacy_unverified", payload));
    }
    let read_recipe = recipe == Some("read") || verifier == "read";
    match verifier {
        "test_report_obtained" => {
            if evidence.readable {
                Ok(pass("report_readable", payload))
            } else {
                Ok(missing("result_unreadable", payload))
            }
        }
        "tests_pass" => {
            if !evidence.readable {
                return Ok(missing("result_unreadable", payload));
            }
            match (evidence.terminal_kind.as_str(), evidence.exit_code) {
                ("settled", Some(0)) => Ok(pass("runner_success", payload)),
                ("settled", Some(_)) => Ok(fail("tests_failed", payload)),
                (_, Some(code)) if code != 0 => Ok(fail("tests_failed", payload)),
                _ => Ok(missing("exit_code_missing", payload)),
            }
        }
        _ if read_recipe => {
            if evidence.readable && evidence.result_digest.is_some() {
                Ok(pass("read_obtained", payload))
            } else {
                Ok(missing("read_unreadable", payload))
            }
        }
        _ => Ok(unknown("verifier_unknown", payload)),
    }
}

fn persist_outcome(
    connection: &Connection,
    task_id: &str,
    run_id: Option<&str>,
    verifier: &str,
    evaluation: &Evaluation,
) -> Result<(), String> {
    let existing: Option<String> = connection
        .query_row(
            "SELECT outcome FROM steward_verifier_outcomes WHERE task_id=?1 AND IFNULL(run_id,'')=IFNULL(?2,'') AND verifier=?3 ORDER BY rowid DESC LIMIT 1",
            params![task_id, run_id, verifier],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    if existing.as_deref() == Some(outcome_key(&evaluation.outcome)) {
        return Ok(());
    }
    connection
        .execute(
            "INSERT INTO steward_verifier_outcomes(id,task_id,run_id,verifier,outcome,reason_code,evidence_json,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                crate::new_id("verifier"),
                task_id,
                run_id,
                verifier,
                outcome_key(&evaluation.outcome),
                evaluation.reason,
                evaluation.evidence.to_string(),
                crate::now_iso()
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

fn pass(reason: &str, evidence: Value) -> Evaluation {
    Evaluation {
        outcome: VerifierOutcome::Pass,
        reason: reason.into(),
        evidence,
    }
}

fn fail(reason: &str, evidence: Value) -> Evaluation {
    Evaluation {
        outcome: VerifierOutcome::Fail,
        reason: reason.into(),
        evidence,
    }
}

fn missing(reason: &str, evidence: Value) -> Evaluation {
    Evaluation {
        outcome: VerifierOutcome::Missing,
        reason: reason.into(),
        evidence,
    }
}

fn unknown(reason: &str, evidence: Value) -> Evaluation {
    Evaluation {
        outcome: VerifierOutcome::Unknown,
        reason: reason.into(),
        evidence,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steward::evidence::{ExecutionEvidence, PRODUCER_HOST_RECIPE, SCHEMA_VERSION};

    fn evidence(exit: Option<i64>, readable: bool, producer: &str) -> ExecutionEvidence {
        ExecutionEvidence {
            schema_version: SCHEMA_VERSION,
            task_id: "task".into(),
            job_id: "job".into(),
            run_id: "run".into(),
            recipe_id: Some("recipe".into()),
            recipe_revision: Some(1),
            recipe_digest: Some("d".into()),
            target_digest: evidence::digest_text("/tmp"),
            terminal_kind: "settled".into(),
            exit_code: exit,
            result_ref: readable.then(|| "report".into()),
            result_digest: readable.then(|| evidence::digest_text("body")),
            producer: producer.into(),
            readable,
            reason_code: "host_observed".into(),
        }
    }

    #[test]
    fn rf5_v_01_job_id_and_model_prose_cannot_pass() {
        let missing_run = evaluate_evidence(
            "test_report_obtained",
            None,
            &ExecutionEvidence {
                result_ref: None,
                result_digest: None,
                readable: false,
                ..evidence(Some(0), false, PRODUCER_HOST_RECIPE)
            },
        )
        .unwrap();
        assert_eq!(missing_run.outcome, VerifierOutcome::Missing);
        let report = evaluate_evidence(
            "test_report_obtained",
            None,
            &evidence(Some(1), true, PRODUCER_HOST_RECIPE),
        )
        .unwrap();
        assert_eq!(report.outcome, VerifierOutcome::Pass);
        let tests = evaluate_evidence(
            "tests_pass",
            None,
            &evidence(Some(1), true, PRODUCER_HOST_RECIPE),
        )
        .unwrap();
        assert_eq!(tests.outcome, VerifierOutcome::Fail);
        let pass = evaluate_evidence(
            "tests_pass",
            None,
            &evidence(Some(0), true, PRODUCER_HOST_RECIPE),
        )
        .unwrap();
        assert_eq!(pass.outcome, VerifierOutcome::Pass);
        let read = evaluate_evidence(
            "read",
            Some("read"),
            &evidence(None, true, PRODUCER_HOST_RECIPE),
        )
        .unwrap();
        assert_eq!(read.outcome, VerifierOutcome::Pass);
    }
}
