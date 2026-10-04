use crate::{database_error, now_iso, AppState};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::path::Path;

pub(super) use super::evidence::snapshot;
use super::verification_process::check;
pub fn finish(state: &AppState, run: &str) -> Result<(), String> {
    let(job,conversation,workspace,exit,result,checks,pending,status):(String,String,String,String,Option<String>,String,bool,String)=state.sqlite_readers.read(|c|c.query_row("SELECT j.id,j.conversation_id,j.workspace_path,t.exit_json,t.result_json,t.checks_json,EXISTS(SELECT 1 FROM terminal_questions q WHERE q.run_id=t.run_id AND q.state='pending'),r.state FROM terminal_runs t JOIN coding_runs r ON r.id=t.run_id JOIN coding_jobs j ON j.id=r.job_id WHERE t.run_id=?1 AND j.current_run_id=t.run_id",[run],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).map_err(database_error))?;
    let exit: Value = serde_json::from_str(&exit).map_err(|_| "exit_receipt_invalid")?;
    if pending && status != "stopping" {
        state.sqlite_writer.write(|c|{
            let tx=c.transaction().map_err(database_error)?;
            settle(&tx,&job,run,"settled","awaiting_user","paused",&json!({"complete":false,"summary":"質問への回答を待っています。CLIの停止を確認しました。"}))?;
            tx.execute("UPDATE terminal_questions SET state='awaiting_user' WHERE run_id=?1 AND state='pending'",[run]).map_err(database_error)?;
            super::questions::schedule(&tx,&job,&conversation,run)?;
            tx.commit().map_err(database_error)
        })?;
        return Ok(());
    }
    let native: Value = result
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let success = exit["code"] == 0
        && exit["stopped"] != true
        && exit["outputError"] != true
        && (native["type"] == "turn.completed"
            || (native["type"] == "result"
                && native["is_error"] != true
                && native["stop_reason"] != "tool_deferred"
                && native["subtype"] == "success"));
    let checks: Vec<Vec<String>> =
        serde_json::from_str(&checks).map_err(|_| "verification_config_invalid")?;
    let mut evidence = Vec::new();
    let interrupted_check = state.sqlite_readers.read(|c| {
        c.query_row(
            "SELECT phase='verifying' FROM terminal_runs WHERE run_id=?1",
            [run],
            |r| r.get::<_, bool>(0),
        )
        .map_err(database_error)
    })?;
    if success && !interrupted_check {
        state.sqlite_writer.write(|c| {
            c.execute(
                "UPDATE terminal_runs SET phase='verifying' WHERE run_id=?1",
                [run],
            )
            .map_err(database_error)?;
            Ok(())
        })?;
        for args in &checks {
            evidence.push(
                check(Path::new(&workspace), args, 120, &|| {
                    state
                        .shutdown_started
                        .load(std::sync::atomic::Ordering::Acquire)
                        || state
                            .sqlite_readers
                            .read(|c| {
                                c.query_row(
                                    "SELECT state='stopping' FROM coding_runs WHERE id=?1",
                                    [run],
                                    |r| r.get::<_, bool>(0),
                                )
                                .map_err(database_error)
                            })
                            .unwrap_or(true)
                })
                .unwrap_or_else(|e| json!({"error":e,"exitCode":null})),
            );
        }
    }
    let after = snapshot(Path::new(&workspace)).unwrap_or_else(|e| json!({"error":e}));
    let candidate: Option<String> = state.sqlite_readers.read(|c| {
        c.query_row(
            "SELECT candidate_json FROM terminal_runs WHERE run_id=?1",
            [run],
            |r| r.get(0),
        )
        .map_err(database_error)
    })?;
    let candidate: Value = candidate
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let status = state.sqlite_readers.read(|c| {
        c.query_row("SELECT state FROM coding_runs WHERE id=?1", [run], |r| {
            r.get::<_, String>(0)
        })
        .map_err(database_error)
    })?;
    let verified = success
        && !interrupted_check
        && status != "stopping"
        && candidate["remainingManualChecks"]
            .as_array()
            .is_some_and(Vec::is_empty)
        && !checks.is_empty()
        && evidence
            .iter()
            .all(|e| e["exitCode"] == 0 && e["timeout"] != true && e["streamIncomplete"] != true)
        && after["error"].is_null();
    let (phase, job_state, run_state) = if status == "stopping" {
        ("cancelled", "interrupted", "interrupted")
    } else if verified {
        ("verified", "completed", "settled")
    } else if success {
        ("review", "awaiting_user", "settled")
    } else {
        ("failed", "failed", "failed")
    };
    let summary = if verified {
        "登録された確認処理はすべて通過しました。画面や動作など、手動確認が必要な部分は確認してください。"
    } else if interrupted_check {
        "完了確認が中断されました。重複実行はせず、ユーザーの確認を待っています。"
    } else if success && checks.is_empty() {
        "実行は終了しました。完了確認の処理が未登録のため、ユーザーの確認を待っています。"
    } else if success {
        "実行は終了しましたが、完了確認が通過していません。確認結果を見て指示してください。"
    } else if status == "stopping" {
        "作業の停止を確認しました。"
    } else {
        "CLIの正常終了を確認できませんでした。作業完了としては扱っていません。"
    };
    let error = if !success {
        native["error"]["message"]
            .as_str()
            .or_else(|| exit["error"].as_str())
            .map(|s| s.chars().take(1000).collect::<String>())
    } else {
        None
    };
    let result = json!({"summary":summary,"error":error,"complete":verified,"verification":{"checks":evidence,"after":after,"interrupted":interrupted_check},"cliResult":native,"candidate":candidate,"exit":exit});
    let mut retry = None;
    state.sqlite_writer.write(|c|{let tx=c.transaction().map_err(database_error)?;settle(&tx,&job,run,run_state,job_state,phase,&result)?;tx.execute("UPDATE terminal_questions SET state='cancelled' WHERE run_id=?1 AND state IN ('pending','awaiting_user')",[run]).map_err(database_error)?;if success && !verified && !interrupted_check && status!="stopping" && evidence.iter().any(|e|e["exitCode"]!=0 || e["timeout"]==true){retry=super::retry::after_checks(&tx,&job,&conversation,run,&result)?;}
        if retry.is_none() && crate::coding::repository::authorize(&tx,&job,&conversation).is_ok(){super::ledger::report(&tx,&conversation,&format!("実装ジョブ {job}（実行 {run}）: {summary}"))?;}tx.commit().map_err(database_error)})?;
    if let Some(run) = retry {
        super::spawn(state, run);
    }
    Ok(())
}
fn settle(
    c: &Connection,
    job: &str,
    run: &str,
    run_state: &str,
    job_state: &str,
    phase: &str,
    result: &Value,
) -> Result<(), String> {
    c.execute(
        "UPDATE coding_runs SET state=?2,result_json=?3,ended_at=?4 WHERE id=?1",
        params![run, run_state, result.to_string(), now_iso()],
    )
    .map_err(database_error)?;
    c.execute(
        "UPDATE coding_jobs SET state=?2,revision=revision+1 WHERE id=?1 AND current_run_id=?3",
        params![job, job_state, run],
    )
    .map_err(database_error)?;
    c.execute(
        "UPDATE terminal_runs SET phase=?2,result_json=?3 WHERE run_id=?1",
        params![run, phase, result.to_string()],
    )
    .map_err(database_error)?;
    crate::coding::repository::event(
        c,
        job,
        run,
        "terminal_settled",
        json!({"phase":phase,"complete":result["complete"]}),
    )
}
