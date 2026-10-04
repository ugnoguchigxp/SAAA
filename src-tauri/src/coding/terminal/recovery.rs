//! Recovery never dispatches a saved prompt. Stop only a live runner or the exact owned child.
use crate::{database_error, now_iso, AppState};
use rusqlite::params;
use saaa_terminal_agent_runtime::{process_identity, read, write_private};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

pub fn block_ingress(state: &AppState, error: &str) -> Result<(), String> {
    let directories=state.sqlite_writer.write(|c|{
        let tx=c.transaction().map_err(database_error)?;
        let mut stmt=tx.prepare("SELECT t.run_id,t.directory,j.id,j.conversation_id FROM terminal_runs t JOIN coding_runs r ON r.id=t.run_id JOIN coding_jobs j ON j.current_run_id=r.id WHERE t.phase IN ('prepared','running','pausing','exited','verifying') AND r.state IN ('starting','running','stopping','outcome_unknown')").map_err(database_error)?;
        let rows=stmt.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;drop(stmt);
        for(run,_,job,conversation)in &rows{
            tx.execute("UPDATE terminal_runs SET phase='blocked' WHERE run_id=?1",[run]).map_err(database_error)?;
            tx.execute("UPDATE coding_runs SET state='outcome_unknown',delivery='unknown',result_json=?2 WHERE id=?1",params![run,json!({"error":error,"complete":false}).to_string()]).map_err(database_error)?;
            tx.execute("UPDATE coding_jobs SET state='outcome_unknown',revision=revision+1 WHERE id=?1",[job]).map_err(database_error)?;
            if crate::coding::repository::authorize(&tx,job,conversation).is_ok(){super::ledger::report(&tx,conversation,&format!("実装ジョブ {job}: 状態の受信に失敗しました。自動再送はせず、停止・確認を待っています。"))?;}
        }
        tx.commit().map_err(database_error)?;Ok(rows.into_iter().map(|r|PathBuf::from(r.1)).collect::<Vec<_>>())
    })?;
    for directory in directories {
        let _ = write_private(&directory.join("cancel"), b"ingress_failed");
    }
    Ok(())
}
pub fn stop_unknown(
    state: &AppState,
    conversation: &str,
    job: &str,
    revision: u64,
) -> Result<Value, String> {
    let (run, directory, nonce) = state.sqlite_writer.write(|c| {
        crate::coding::repository::authorize(c, job, conversation)?;
        let (status, run) = crate::coding::repository::revision(c, job, revision)?;
        if status != "outcome_unknown" {
            return Err("terminal_recovery_not_required".into());
        }
        let (directory, nonce): (String, String) = c
            .query_row(
                "SELECT directory,nonce FROM terminal_runs WHERE run_id=?1",
                [&run],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(database_error)?;
        Ok((run, PathBuf::from(directory), nonce))
    })?;
    let spec = read(&directory)?;
    if spec.run != run || spec.nonce != nonce {
        return Err("terminal_receipt_scope_mismatch".into());
    }
    write_private(&directory.join("cancel"), b"user_requested_recovery_stop")?;
    let path = directory.join("child.json");
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| "terminal_child_identity_missing_manual_stop_required")?;
    if metadata.file_type().is_symlink() || metadata.len() > 16000 {
        return Err("terminal_child_identity_invalid".into());
    }
    let receipt: Value = serde_json::from_slice(
        &std::fs::read(path).map_err(|_| "terminal_child_identity_missing")?,
    )
    .map_err(|_| "terminal_child_identity_invalid")?;
    if receipt["run"] != run || receipt["nonce"] != nonce {
        return Err("terminal_child_identity_scope_mismatch".into());
    }
    let pid = receipt["pid"]
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 1)
        .ok_or("terminal_child_identity_invalid")?;
    let saved = receipt["identity"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("terminal_child_identity_missing")?;
    // A live runner handles its own child group. Allow its durable cancellation first.
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_may_exist(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    if process_may_exist(pid) {
        if process_identity(pid) != saved {
            return Err("old_process_identity_mismatch_manual_stop_required".into());
        }
        #[cfg(unix)]
        {
            if unsafe { libc::getpgid(pid as i32) } != pid as i32 {
                return Err("terminal_process_group_unverified".into());
            }
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
            std::thread::sleep(Duration::from_millis(500));
            if process_may_exist(pid) {
                if process_identity(pid) != saved {
                    return Err("old_process_identity_changed".into());
                }
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
        }
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while process_may_exist(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    if process_may_exist(pid) {
        return Err("terminal_containment_failed_manual_stop_required".into());
    }
    state.sqlite_writer.write(|c|{
        let tx=c.transaction().map_err(database_error)?;
        crate::coding::repository::revision(&tx,job,revision)?;
        tx.execute("UPDATE coding_runs SET state='interrupted',ended_at=?2,result_json=?3 WHERE id=?1",params![run,now_iso(),json!({"complete":false,"summary":"対象プロセスの停止を確認しました。元の結果は未確認です。","stopVerified":true}).to_string()]).map_err(database_error)?;
        tx.execute("UPDATE terminal_runs SET phase='cancelled' WHERE run_id=?1",[&run]).map_err(database_error)?;
        tx.execute("UPDATE coding_jobs SET state='interrupted',revision=revision+1 WHERE id=?1",[job]).map_err(database_error)?;
        tx.execute("UPDATE terminal_questions SET state='cancelled' WHERE job_id=?1 AND state IN ('pending','awaiting_user')",[job]).map_err(database_error)?;
        super::ledger::report(&tx,conversation,&format!("実装ジョブ {job}: 対象プロセスの停止を確認しました。作業結果は未確認です。"))?;
        tx.commit().map_err(database_error)?;Ok(json!({"stopped":true,"complete":false}))
    })
}
fn process_may_exist(pid: u32) -> bool {
    #[cfg(unix)]
    {
        (unsafe { libc::kill(pid as i32, 0) }) == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// A prepared receipt may be left by an app crash. Never replay its launch.
pub(super) fn unlaunched(
    writer: &crate::persistence::SqliteWriter,
    run: &str,
) -> Result<bool, String> {
    writer.write(|c|{
        let tx=c.transaction().map_err(database_error)?;
        let row=tx.query_row("SELECT j.id,j.conversation_id,r.delivery FROM terminal_runs t JOIN coding_runs r ON r.id=t.run_id JOIN coding_jobs j ON j.current_run_id=r.id WHERE t.run_id=?1 AND t.phase='prepared' AND (julianday('now')-julianday(t.created_at))*86400>60",[run],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)));
        let (job,conversation,delivery)=match row{Ok(row)=>row,Err(rusqlite::Error::QueryReturnedNoRows)=>return Ok(false),Err(error)=>return Err(database_error(error))};
        let status=if delivery=="prepared"{"failed"}else{"outcome_unknown"};
        tx.execute("UPDATE coding_runs SET state=?2,result_json=?3 WHERE id=?1",params![run,status,json!({"complete":false,"error":"terminal_launch_receipt_incomplete"}).to_string()]).map_err(database_error)?;
        tx.execute("UPDATE terminal_runs SET phase=?2 WHERE run_id=?1",params![run,if status=="failed"{"failed"}else{"blocked"}]).map_err(database_error)?;
        tx.execute("UPDATE coding_jobs SET state=?2,revision=revision+1 WHERE id=?1",params![job,status]).map_err(database_error)?;
        if crate::coding::repository::authorize(&tx,&job,&conversation).is_ok(){super::ledger::report(&tx,&conversation,&format!("実装ジョブ {job}: 起動の記録が中断されています。自動再送せず、確認を待っています。"))?;}
        tx.commit().map_err(database_error)?;Ok(true)
    })
}
