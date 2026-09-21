use super::{
    process::{self, Process},
    session_reader,
};
use crate::{
    coding::{contracts::CodingSettings, repository as repo},
    database_error,
    persistence::SqliteWriter,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

pub fn run(writer: Arc<SqliteWriter>, run: String) {
    let _personal_slot = crate::memory::personal_state::worker::blocking_generation();
    let result = execute(&writer, &run);
    let committed = writer.write(|c| {
        let tx = c.transaction().map_err(database_error)?;
        let (job, delivery, status): (String, String, String) = tx
            .query_row(
                "SELECT job_id,delivery,state FROM coding_runs WHERE id=?1",
                [&run],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(database_error)?;
        let (state, value) = match result {
            Ok(value) => (
                if status == "stopping" {
                    "interrupted"
                } else if value["modelErrors"].as_u64().unwrap_or(0) > 0 {
                    "failed"
                } else {
                    "settled"
                },
                value,
            ),
            Err(error) => (
                if status == "stopping" {
                    "interrupted"
                } else {
                    "failed"
                },
                json!({"error":error,"complete":false}),
            ),
        };
        let delivery = if delivery == "sending" {
            "unknown"
        } else if delivery == "prepared" {
            "rejected"
        } else {
            &delivery
        };
        tx.execute(
            "UPDATE coding_runs SET state=?2,delivery=?3,result_json=?4,ended_at=?5 WHERE id=?1",
            params![run, state, delivery, value.to_string(), crate::now_iso()],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE coding_jobs SET state=?2,revision=revision+1 WHERE id=?1",
            params![job, state],
        )
        .map_err(database_error)?;
        repo::event(
            &tx,
            &job,
            &run,
            state,
            json!({"complete":value["complete"]}),
        )?;
        if let Ok(Some(task_id)) = tx
            .query_row(
                "SELECT id FROM steward_tasks WHERE coding_job_id=?1 LIMIT 1",
                [&job],
                |row| row.get::<_, String>(0),
            )
            .optional()
        {
            let last_entry = value["lastEntryId"].as_str();
            let host_body = serde_json::json!({
                "lastEntryId": value.get("lastEntryId"),
                "complete": value.get("complete"),
                "exitCode": value.get("exitCode"),
                "tools": value.get("tools"),
            })
            .to_string();
            let evidence = crate::steward::evidence::from_host_session(
                &task_id,
                &job,
                &run,
                state,
                workspace_for_run(&tx, &run).as_deref().unwrap_or(""),
                last_entry,
                &host_body,
                None,
                None,
                None,
                value["exitCode"].as_i64(),
                crate::steward::evidence::PRODUCER_HOST_PI,
            );
            let _ = crate::steward::evidence::persist(&tx, &evidence);
        }
        crate::steward::driver::consume(&tx)?;
        tx.commit().map_err(database_error)
    });
    if committed.is_ok() {
        crate::steward::pump::signal_committed();
    }
}

fn workspace_for_run(connection: &rusqlite::Connection, run: &str) -> Option<String> {
    connection
        .query_row(
            "SELECT j.workspace_path FROM coding_runs r JOIN coding_jobs j ON j.id=r.job_id WHERE r.id=?1",
            [run],
            |row| row.get(0),
        )
        .ok()
}
fn execute(writer: &SqliteWriter, run: &str) -> Result<Value, String> {
    let (job,workspace,session,settings,payload,binding):(String,String,String,String,String,Option<String>)=writer.read_serialized(|c|c.query_row("SELECT j.id,j.workspace_path,j.session_path,j.settings_json,r.payload,j.session_id FROM coding_runs r JOIN coding_jobs j ON j.id=r.job_id WHERE r.id=?1",[run],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).map_err(database_error))?;
    let settings: CodingSettings =
        serde_json::from_str(&settings).map_err(|_| "coding_settings_invalid")?;
    process::check_settings(&settings)?;
    let workspace = PathBuf::from(workspace);
    if std::fs::canonicalize(&workspace).map_err(|_| "workspace_missing")? != workspace {
        return Err("workspace_changed".into());
    }
    let session = PathBuf::from(session);
    let boundary = if let Some(binding) = binding.as_ref() {
        let saved = session_reader::read(&session, &workspace, None)?;
        if &saved.id != binding {
            return Err("session_binding_mismatch".into());
        }
        saved.leaf
    } else {
        None
    };
    if stopping(writer, run)? {
        return Err("cancelled_before_start".into());
    }
    writer.write(|c| {
        c.execute(
            "UPDATE coding_runs SET process_identity='launching' WHERE id=?1",
            [run],
        )
        .map_err(database_error)?;
        Ok(())
    })?;
    let mut child = Process::open(&settings, &workspace, &session)?;
    writer.write(|c| {
        c.execute(
            "UPDATE coding_runs SET pid=?2,process_identity=?3,boundary=?4 WHERE id=?1",
            params![run, child.pid(), identity(child.pid()), boundary],
        )
        .map_err(database_error)?;
        repo::event(c, &job, run, "process_started", json!({"pid":child.pid()}))
    })?;
    let outcome: Result<usize, String> = (|| {
        let ready =
            process::ready_cancellable(&mut child, &settings, &session, || stopping(writer, run))?;
        if binding.as_ref().is_some_and(|id| ready["sessionId"] != *id) {
            return Err("session_binding_mismatch".into());
        }
        let prompt = writer.write(|c| {
            if !source_valid(c, run)? {
                return Err("source_unavailable".into());
            }
            c.execute(
                "UPDATE coding_jobs SET session_id=?2 WHERE id=?1",
                params![job, ready["sessionId"].as_str()],
            )
            .map_err(database_error)?;
            c.execute(
                "UPDATE coding_runs SET delivery='sending' WHERE id=?1 AND state='starting'",
                [run],
            )
            .map_err(database_error)
            .and_then(|n| {
                if n == 1 {
                    child.send("prompt", json!({"message":payload}))
                } else {
                    Err("cancelled_before_send".into())
                }
            })
        })?;

        let deadline = Instant::now() + Duration::from_secs(1800);
        let acceptance = Instant::now() + Duration::from_secs(30);
        let mut accepted = false;
        let mut settled = false;
        let mut stop_deadline = None;
        let mut errors = 0;
        while !settled {
            if stop_deadline.is_none() && (stopping(writer, run)? || Instant::now() > deadline) {
                child.send("clear_queue", json!({}))?;
                child.send("abort", json!({}))?;
                stop_deadline = Some(Instant::now() + Duration::from_secs(5));
            }
            if stop_deadline.is_some_and(|d| Instant::now() > d) {
                return Err("pi_abort_timeout".into());
            }
            if !accepted && Instant::now() > acceptance {
                return Err("rpc_acceptance_timeout".into());
            }
            if let Some(event) = child.next(Duration::from_millis(100))? {
                if event["type"] == "response" && event["id"] == prompt {
                    if event["success"] != true {
                        return Err("prompt_rejected".into());
                    }
                    accepted = true;
                    writer.write(|c|{c.execute("UPDATE coding_runs SET delivery='accepted',state=CASE WHEN state='stopping' THEN state ELSE 'running' END WHERE id=?1",[run]).map_err(database_error)?;c.execute("UPDATE coding_jobs SET state=CASE WHEN state='cancel_requested' THEN state ELSE 'running' END WHERE id=?1",[&job]).map_err(database_error)?;repo::event(c,&job,run,"accepted",json!({}))})?;
                }
                if event["type"] == "agent_settled" {
                    settled = true;
                }
                if event["type"] == "tool_execution_end" && event["isError"] == true {
                    errors += 1;
                }
            }
        }
        if !accepted {
            return Err("settled_without_acceptance".into());
        }
        Ok(errors)
    })();
    if outcome.is_err() {
        let _ = child.send("clear_queue", json!({}));
        let _ = child.send("abort", json!({}));
    }
    let closed = child.close();
    let errors = outcome?;
    closed?;
    let result = session_reader::read(&session, &workspace, boundary.as_deref())?;
    if result.leaf.as_deref() == boundary.as_deref() {
        return Err("session_result_missing".into());
    }
    Ok(
        json!({"summary":result.summary,"errors":result.errors+errors,"modelErrors":result.model_errors,"tools":result.tools,"lastEntryId":result.leaf,"sessionId":result.id,"complete":result.complete,"truncated":!result.complete,"meaning":"execution ended; goal achievement is not certified"}),
    )
}
fn source_valid(c: &rusqlite::Connection, run: &str) -> Result<bool, String> {
    c.query_row("SELECT EXISTS(
        SELECT 1 FROM coding_runs r
        JOIN coding_jobs j ON j.id=r.job_id
        LEFT JOIN coding_origin_bindings o ON o.job_id=j.id
        WHERE r.id=?1 AND (
          (o.origin_kind='user_turn' AND EXISTS(
            SELECT 1 FROM conversation_messages m WHERE m.id=o.origin_id AND m.conversation_id=j.conversation_id
          ) AND NOT EXISTS(
            SELECT 1 FROM coding_runs prior
            LEFT JOIN conversation_messages source ON source.id=prior.source_id
            WHERE prior.job_id=j.id AND source.id IS NULL
          )) OR
          (o.origin_kind='delegated_event' AND EXISTS(
            SELECT 1 FROM steward_tasks t
            JOIN steward_delegations d ON d.id=t.delegation_id
            JOIN steward_goals g ON g.id=d.goal_id
            JOIN conversation_messages m ON m.id=t.source_id
            WHERE t.id=o.origin_id AND t.conversation_id=j.conversation_id
              AND t.loop_state IN ('queued','running','awaiting_user')
              AND d.status='active' AND d.superseded_by IS NULL
              AND g.status='active' AND g.superseded_by IS NULL
              AND m.conversation_id=j.conversation_id
          )) OR
          (o.job_id IS NULL AND EXISTS(
            SELECT 1 FROM conversation_messages m WHERE m.id=j.source_id AND m.conversation_id=j.conversation_id
          ) AND NOT EXISTS(
            SELECT 1 FROM coding_runs prior
            LEFT JOIN conversation_messages source ON source.id=prior.source_id
            WHERE prior.job_id=j.id AND source.id IS NULL
          ))
        ))",[run],|r|r.get(0)).map_err(database_error)
}
fn stopping(writer: &SqliteWriter, run: &str) -> Result<bool, String> {
    writer.write(|c| {
        let status: String = c
            .query_row("SELECT state FROM coding_runs WHERE id=?1", [run], |r| {
                r.get(0)
            })
            .map_err(database_error)?;
        if !source_valid(c,run)? {
            c.execute("UPDATE coding_runs SET state='stopping',stop_reason='source_unavailable' WHERE id=?1",[run]).map_err(database_error)?;
            c.execute("UPDATE coding_jobs SET state='cancel_requested' WHERE current_run_id=?1",[run]).map_err(database_error)?;
            return Ok(true);
        }
        Ok(status == "stopping")
    })
}
pub fn identity(pid: u32) -> String {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "lstart=", "-o", "command="])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}
