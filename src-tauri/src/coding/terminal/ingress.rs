use crate::{database_error, now_iso, AppState};
use rusqlite::{params, Connection};
use saaa_terminal_agent_runtime::{read, write_private, Event};
use std::{
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::PathBuf,
};

pub fn drain(state: &AppState) -> Result<bool, String> {
    let runs=state.sqlite_readers.read(|c|{
        let mut stmt=c.prepare("SELECT t.run_id,t.directory,t.offset,t.phase,r.state,t.nonce FROM terminal_runs t JOIN coding_runs r ON r.id=t.run_id WHERE t.phase IN ('prepared','running','pausing','exited','verifying')").map_err(database_error)?;
        let rows=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,u64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?))).map_err(database_error)?;
        rows.collect::<Result<Vec<_>,_>>().map_err(database_error)
    })?;
    let mut changed = false;
    for (run, directory, offset, phase, status, nonce) in runs {
        let directory = PathBuf::from(directory);
        if phase == "prepared"
            && !directory.join("launched").exists()
            && super::recovery::unlaunched(&state.sqlite_writer, &run)?
        {
            changed = true;
            continue;
        }
        if status == "stopping" || status == "outcome_unknown" {
            let _ = write_private(&directory.join("cancel"), b"host_stop");
        }
        let authorized=state.sqlite_readers.read(|c|{let(job,conversation):(String,String)=c.query_row("SELECT j.id,j.conversation_id FROM coding_jobs j JOIN coding_runs r ON r.job_id=j.id WHERE r.id=?1",[&run],|r|Ok((r.get(0)?,r.get(1)?))).map_err(database_error)?;Ok(crate::coding::repository::authorize(c,&job,&conversation).is_ok())})?;
        if !authorized {
            let _ = write_private(&directory.join("cancel"), b"source_unavailable");
            state.sqlite_writer.write(|c|{c.execute("UPDATE coding_runs SET state='stopping',stop_reason='source_unavailable' WHERE id=?1 AND state IN ('starting','running')",[&run]).map_err(database_error)?;Ok(())})?;
        }
        let spec = match read(&directory) {
            Ok(spec) => spec,
            Err(_) if phase == "prepared" && !directory.join("spec.json").exists() => continue,
            Err(error) => return Err(error),
        };
        if spec.run != run || spec.nonce != nonce {
            return Err("terminal_receipt_scope_mismatch".into());
        }
        if let Ok(mut file) = std::fs::File::open(directory.join("events.jsonl")) {
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| e.to_string())?;
            let mut input = BufReader::new(file);
            let mut next = offset;
            for _ in 0..128 {
                let mut line = Vec::new();
                let n = input
                    .by_ref()
                    .take(65538)
                    .read_until(b'\n', &mut line)
                    .map_err(|e| e.to_string())?;
                if n > 65537 {
                    return Err("terminal_event_invalid".into());
                }
                if n == 0 || !line.ends_with(b"\n") {
                    break;
                }
                next += n as u64;
                let event: Event =
                    serde_json::from_slice(&line).map_err(|_| "terminal_event_invalid")?;
                if event.run != run || event.nonce != spec.nonce {
                    return Err("terminal_event_scope_mismatch".into());
                }
                let wake = state.sqlite_writer.write(|c| {
                    let tx = c.transaction().map_err(database_error)?;
                    let wake = consume(&tx, &event, next)?;
                    tx.commit().map_err(database_error)?;
                    Ok(wake)
                })?;
                if wake && event.kind == "question" && event.data["kind"] != "question" {
                    let _ = write_private(&directory.join("cancel"), b"question_persisted");
                }
                changed |= wake;
            }
        }
        // The spool commits first. A restart recovers the same immutable receipt.
        let current = state.sqlite_readers.read(|c| {
            c.query_row(
                "SELECT phase FROM terminal_runs WHERE run_id=?1",
                [&run],
                |r| r.get::<_, String>(0),
            )
            .map_err(database_error)
        })?;
        if current == "exited" || current == "verifying" {
            super::verify::finish(state, &run)?;
            changed = true;
        }
        if phase == "pausing" && !directory.join("cancel").exists() {
            // A batched Claude AskUserQuestion may ignore defer. Bound that fallback.
            let age=state.sqlite_readers.read(|c|c.query_row("SELECT COALESCE((julianday('now')-julianday(MAX(created_at)))*86400,0) FROM terminal_questions WHERE run_id=?1",[&run],|r|r.get::<_,f64>(0)).map_err(database_error))?;
            if age > 10.0 {
                let _ = write_private(&directory.join("cancel"), b"defer_timeout");
            }
        }
        let drained = state.sqlite_readers.read(|c| {
            c.query_row(
                "SELECT offset FROM terminal_runs WHERE run_id=?1",
                [&run],
                |r| r.get::<_, u64>(0),
            )
            .map_err(database_error)
        })? >= std::fs::metadata(directory.join("events.jsonl"))
            .map(|m| m.len())
            .unwrap_or(0);
        if drained
            && !matches!(
                current.as_str(),
                "exited" | "verifying" | "paused" | "verified" | "review" | "failed"
            )
            && directory.join("launched").exists()
        {
            let lock = std::fs::OpenOptions::new()
                .write(true)
                .open(directory.join("runner.lock"));
            if let Ok(lock) = lock {
                if fs2::FileExt::try_lock_exclusive(&lock).is_ok() {
                    // Lock is released only when the independent runner exits. No automatic resend.
                    super::recovery::block_ingress(
                        state,
                        "terminal_runner_exited_without_receipt",
                    )?;
                    changed = true;
                }
            }
        }
    }
    Ok(changed)
}
fn consume(c: &Connection, event: &Event, next: u64) -> Result<bool, String> {
    let inserted = c
        .execute(
            "INSERT OR IGNORE INTO terminal_inbox(id,run_id,kind) VALUES(?1,?2,?3)",
            params![event.id, event.run, event.kind],
        )
        .map_err(database_error)?;
    c.execute(
        "UPDATE terminal_runs SET offset=?2 WHERE run_id=?1",
        params![event.run, next],
    )
    .map_err(database_error)?;
    if inserted == 0 {
        return Ok(false);
    }
    let(job,current):(String,String)=c.query_row("SELECT j.id,j.current_run_id FROM coding_jobs j JOIN coding_runs r ON r.job_id=j.id WHERE r.id=?1",[&event.run],|r|Ok((r.get(0)?,r.get(1)?))).map_err(database_error)?;
    if current != event.run {
        return Ok(false);
    }
    match event.kind.as_str() {
        "process" => {
            c.execute("UPDATE coding_runs SET state=CASE WHEN state='stopping' THEN state ELSE 'running' END,pid=?2,process_identity=?3 WHERE id=?1",params![event.run,event.data["pid"].as_u64(),event.data["identity"].as_str()]).map_err(database_error)?;
            c.execute("UPDATE coding_jobs SET state=CASE WHEN state='cancel_requested' THEN state ELSE 'running' END WHERE id=?1",[&job]).map_err(database_error)?;
        }
        "output" => {
            let value = &event.data;
            if let Some(id) = value["session_id"]
                .as_str()
                .or_else(|| value["thread_id"].as_str())
            {
                let prior: Option<String> = c
                    .query_row(
                        "SELECT session_id FROM coding_jobs WHERE id=?1",
                        [&job],
                        |r| r.get(0),
                    )
                    .map_err(database_error)?;
                if prior.as_ref().is_some_and(|prior| prior != id) {
                    return Err("session_binding_mismatch".into());
                }
                if id.len() > 160 || id.is_empty() {
                    return Err("session_binding_invalid".into());
                }
                c.execute(
                    "UPDATE coding_jobs SET session_id=?2 WHERE id=?1",
                    params![job, id],
                )
                .map_err(database_error)?;
                c.execute(
                    "UPDATE coding_runs SET delivery='accepted' WHERE id=?1",
                    [&event.run],
                )
                .map_err(database_error)?;
            }
            if value["type"] == "result"
                || value["type"] == "turn.completed"
                || value["type"] == "turn.failed"
            {
                c.execute(
                    "UPDATE terminal_runs SET result_json=?2 WHERE run_id=?1",
                    params![event.run, value.to_string()],
                )
                .map_err(database_error)?;
            }
            crate::coding::repository::event(
                c,
                &job,
                &event.run,
                "terminal_output",
                value.clone(),
            )?;
        }
        "question" => {
            let id = event.data["questionId"]
                .as_str()
                .ok_or("question_id_missing")?;
            let kind = event.data["kind"].as_str().ok_or("question_kind_missing")?;
            if !matches!(kind, "question" | "permission" | "blocker") || id.len() > 160 {
                return Err("question_invalid".into());
            }
            // Identity is host run + CLI tool-use id, preventing collisions across resumptions.
            let id = format!("{}:{id}", event.run);
            c.execute("INSERT OR IGNORE INTO terminal_questions(id,run_id,job_id,kind,input_json,state,created_at) VALUES(?1,?2,?3,?4,?5,'pending',?6)",params![id,event.run,job,kind,event.data["input"].to_string(),now_iso()]).map_err(database_error)?;
            c.execute(
                "UPDATE terminal_runs SET phase='pausing' WHERE run_id=?1",
                [&event.run],
            )
            .map_err(database_error)?;
            c.execute(
                "UPDATE coding_jobs SET state='awaiting_user',revision=revision+1 WHERE id=?1",
                [&job],
            )
            .map_err(database_error)?;
        }
        "exit" => {
            c.execute(
                "UPDATE terminal_runs SET phase='exited',exit_json=?2 WHERE run_id=?1",
                params![event.run, event.data.to_string()],
            )
            .map_err(database_error)?;
        }
        "candidate" => {
            c.execute(
                "UPDATE terminal_runs SET candidate_json=?2 WHERE run_id=?1",
                params![event.run, event.data.to_string()],
            )
            .map_err(database_error)?;
        }
        "hook" => {
            crate::coding::repository::event(
                c,
                &job,
                &event.run,
                "terminal_hook",
                event.data.clone(),
            )?;
        }
        "stderr" => {
            crate::coding::repository::event(
                c,
                &job,
                &event.run,
                "terminal_diagnostic",
                event.data.clone(),
            )?;
        }
        _ => return Err("terminal_event_kind_invalid".into()),
    }
    Ok(true)
}
#[cfg(test)]
pub(super) fn consume_test(c: &Connection, event: &Event, next: u64) -> Result<bool, String> {
    consume(c, event, next)
}
