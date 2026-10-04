use crate::{database_error, now_iso, persistence::SqliteWriter, AppState};
use rusqlite::{params, OptionalExtension};
use saaa_terminal_agent_runtime as runtime;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};
use tauri::{Emitter, Manager};

pub fn spawn(state: &AppState, run: String) {
    let writer = state.sqlite_writer.clone();
    let root = state.data_directory.join("terminal-agents");
    std::thread::spawn(move || {
        if let Err(error) = prepare(&writer, &root, &run) {
            let _ = fail(&writer, &run, &error);
        }
    });
}
fn prepare(writer: &SqliteWriter, root: &std::path::Path, run: &str) -> Result<(), String> {
    let (job,workspace,payload,settings,session,answer):(String,String,String,String,Option<String>,Option<String>)=writer.read_serialized(|c|c.query_row("SELECT j.id,j.workspace_path,r.payload,j.settings_json,j.session_id,(SELECT q.answer_json FROM terminal_resumes x JOIN terminal_questions q ON q.id=x.question_id WHERE x.source_id=r.source_id) FROM coding_runs r JOIN coding_jobs j ON j.id=r.job_id WHERE r.id=?1",[run],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).map_err(database_error))?;
    let settings: crate::coding::contracts::CodingSettings =
        serde_json::from_str(&settings).map_err(|_| "terminal_settings_invalid")?;
    let workspace = PathBuf::from(workspace);
    if std::fs::canonicalize(&workspace).map_err(|_| "workspace_missing")? != workspace {
        return Err("workspace_changed".into());
    }
    let mut answer = answer
        .map(|s| serde_json::from_str::<Value>(&s))
        .transpose()
        .map_err(|_| "decision_invalid")?;
    if answer.is_none() {
        let cancelled=writer.read_serialized(|c|c.query_row("SELECT input_json FROM terminal_questions WHERE job_id=?1 AND kind='question' AND state='cancelled' ORDER BY rowid DESC LIMIT 1",[&job],|r|r.get::<_,String>(0)).optional().map_err(database_error))?;
        if let Some(input) = cancelled {
            answer = Some(
                json!({"kind":"question_cancel","input":serde_json::from_str::<Value>(&input).map_err(|_|"question_invalid")?}),
            );
        }
    }
    let spec = super::launch::spec(
        &settings,
        job.clone(),
        run.into(),
        workspace.clone(),
        payload,
        session,
        answer,
    )?;
    let directory = root.join(run);
    let baseline = super::verify::snapshot(&workspace)?;
    // A durable receipt precedes all OS process launch. Restart never resends it.
    writer.write(|c|{
        let tx=c.transaction().map_err(database_error)?;
        let status:String=tx.query_row("SELECT state FROM coding_runs WHERE id=?1",[run],|r|r.get(0)).map_err(database_error)?;
        if status!="starting" {return Err("cancelled_before_start".into());}
        if !crate::runtime::pi::runner::source_valid(&tx,run)? && !super::ledger::source_authorized(&tx,&tx.query_row("SELECT source_id FROM coding_runs WHERE id=?1",[run],|r|r.get::<_,String>(0)).map_err(database_error)?,&job)? {return Err("source_unavailable".into());}
        tx.execute("INSERT INTO terminal_runs(run_id,directory,nonce,phase,cli,terminal,baseline_json,checks_json,created_at) VALUES(?1,?2,?3,'prepared',?4,?5,?6,?7,?8)",params![run,directory.to_string_lossy(),spec.nonce,settings.terminal_cli,settings.terminal_kind,baseline.to_string(),serde_json::to_string(&settings.terminal_checks).unwrap(),now_iso()]).map_err(database_error)?;
        tx.commit().map_err(database_error)
    })?;
    runtime::create(&directory, &spec)?;
    runtime::configuration(&directory, &spec)?;
    super::launch::viewer(&settings.terminal_kind, &directory, &spec.helper)?;
    writer.write(|c|{ c.execute("UPDATE coding_runs SET process_identity='launching',delivery='sending' WHERE id=?1 AND state='starting'",[run]).map_err(database_error).and_then(|n|if n==1{Ok(())}else{Err("cancelled_before_send".into())}) })?;
    let mut child = Command::new(&spec.helper)
        .args(["--saaa-terminal-agent", "run"])
        .arg(&directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "terminal_runner_launch_failed")?;
    let saved=writer.write(|c|{
        c.execute("UPDATE terminal_runs SET runner_pid=?2,runner_identity=?3,phase=CASE WHEN phase='prepared' THEN 'running' ELSE phase END WHERE run_id=?1",params![run,child.id(),crate::runtime::pi::runner::identity(child.id())]).map_err(database_error)?;Ok(())
    });
    if saved.is_err() {
        let _ = runtime::write_private(&directory.join("cancel"), b"host_receipt_failed");
    }
    // Reap only the child launched here. Recovery never kills by an old PID.
    let status = child.wait().map_err(|_| "terminal_runner_wait_failed")?;
    saved?;
    if !status.success() {
        return Err("terminal_runner_failed_or_delivery_unknown".into());
    }
    Ok(())
}
fn fail(writer: &SqliteWriter, run: &str, error: &str) -> Result<(), String> {
    writer.write(|c|{
        let tx=c.transaction().map_err(database_error)?;
        let(job,conversation,delivery):(String,String,String)=tx.query_row("SELECT j.id,j.conversation_id,r.delivery FROM coding_runs r JOIN coding_jobs j ON j.id=r.job_id WHERE r.id=?1",[run],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(database_error)?;
        let status=if delivery=="prepared"{"failed"}else{"outcome_unknown"};
        let changed=tx.execute("UPDATE coding_runs SET state=?2,result_json=?3 WHERE id=?1 AND state IN ('starting','running','stopping')",params![run,status,json!({"error":error,"complete":false}).to_string()]).map_err(database_error)?;
        if changed==0{return Ok(());}
        tx.execute("UPDATE terminal_runs SET phase=CASE WHEN ?2='failed' THEN 'failed' ELSE 'blocked' END WHERE run_id=?1",params![run,status]).map_err(database_error)?;
        tx.execute("UPDATE coding_jobs SET state=?2,revision=revision+1 WHERE id=?1 AND current_run_id=?3",params![job,status,run]).map_err(database_error)?;
        super::ledger::report(&tx,&conversation,&format!("実装ジョブ {job}: {error}。完了は確認できていません。"))?;
        tx.commit().map_err(database_error)
    })?;
    crate::steward::pump::signal_committed();
    Ok(())
}
static WAKE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
pub(super) fn wake_path() -> Option<PathBuf> {
    WAKE.get().cloned()
}
pub fn start<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    #[cfg(unix)]
    let socket = tempfile::Builder::new()
        .prefix("saaa-ta-")
        .tempdir_in("/tmp")
        .ok()
        .and_then(|directory| {
            let path = directory.path().join("wake");
            let socket = std::os::unix::net::UnixDatagram::bind(&path).ok()?;
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .ok()?;
            let _ = WAKE.set(path);
            Some((directory, socket))
        });
    std::thread::spawn(move || loop {
        let state = app.state::<AppState>();
        match super::ingress::drain(&state) {
            Ok(true) => {
                let _ = app.emit("conversation-queue-updated", ());
                let _ = app.emit(
                    "delegated-report-committed",
                    json!({"conversationId":crate::PRIMARY_CONVERSATION_ID}),
                );
                state.conversation_queue_wake.notify_one();
                crate::steward::pump::signal_committed();
            }
            Err(error) => {
                eprintln!("terminal ingress: {error}");
                let _ = super::recovery::block_ingress(&state, &error);
                crate::steward::pump::signal_committed();
            }
            _ => {}
        }
        #[cfg(unix)]
        if let Some((_, socket)) = &socket {
            let mut message = [0u8; 512];
            let valid=socket.recv(&mut message).ok().and_then(|n|serde_json::from_slice::<Value>(&message[..n]).ok()).is_some_and(|wake|state.sqlite_readers.read(|c|c.query_row("SELECT EXISTS(SELECT 1 FROM terminal_runs WHERE run_id=?1 AND nonce=?2)",params![wake["run"].as_str(),wake["nonce"].as_str()],|r|r.get::<_,bool>(0)).map_err(database_error)).unwrap_or(false));
            if valid {
                continue;
            }
        } else {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        #[cfg(not(unix))]
        std::thread::sleep(std::time::Duration::from_secs(2));
    });
}
