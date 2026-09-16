use super::{contracts::*, repository as repo};
use crate::{database_error, new_id, now_iso, AppState, StartTurnInput};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};

pub fn execute(
    state: &AppState,
    input: &StartTurnInput,
    call: &crate::runtime::agent_tools::AgentToolCall,
) -> Result<Value, String> {
    validate(&call.name, &call.arguments)?;
    if state
        .shutdown_started
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err("app_stopping".into());
    }
    let canonical: Value =
        serde_json::from_str(&call.arguments).map_err(|_| "invalid_arguments")?;
    let digest = format!("{:x}", Sha256::digest(format!("{}:{canonical}", call.name)));
    // Replays must not probe or create a second process.
    let replay = state.sqlite_readers.read(|c| {
        let source = repo::source(c, &input.conversation_id, &input.run_id)?;
        if let Some(value)=repo::cached(c, &source, &call.id, &digest)? {return Ok(Some(value));}
        if call.name=="coding_start" {
            let prior:Option<(String,String)>=c.query_row("SELECT j.id,r.digest FROM coding_jobs j JOIN coding_runs r ON r.job_id=j.id WHERE j.source_id=?1 ORDER BY r.rowid LIMIT 1",[&source],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(database_error)?;
            if let Some((job,prior_digest))=prior {
                if digest!=prior_digest {return Err("idempotency_conflict".into());}
                let mut value=repo::inspect(c,&input.conversation_id,&job,0,1)?;
                value["accepted"]=json!(true);return Ok(Some(value));
            }
        }
        Ok(None)
    })?;
    if let Some(value) = replay {
        return Ok(value);
    }
    if matches!(call.name.as_str(), "coding_start" | "coding_continue") {
        let settings = state.sqlite_readers.read(repo::settings)?;
        if !settings.enabled {
            return Err("coding_disabled".into());
        }
        state.sqlite_readers.read(|c| {
            let busy:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM coding_runs WHERE state IN ('starting','running','stopping','outcome_unknown'))",[],|r|r.get(0)).map_err(database_error)?;
            if busy{return Err("busy".into());}
            if call.name=="coding_start" {
                let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM coding_workspaces WHERE id=?1 AND conversation_id=?2)",params![canonical["workspaceId"].as_str(),input.conversation_id],|r|r.get(0)).map_err(database_error)?;
                if !valid{return Err("workspace_required".into());}
            } else {
                let job=canonical["jobId"].as_str().ok_or("invalid_arguments")?;
                repo::authorize(c,job,&input.conversation_id)?;
                repo::revision(c,job,canonical["expectedRevision"].as_u64().ok_or("invalid_arguments")?)?;
            }
            Ok(())
        })?;
        crate::runtime::pi::process::probe(&settings, &state.data_directory)?;
    }
    let mut launch = None;
    let result=state.sqlite_writer.write(|c|{
        if state.shutdown_started.load(std::sync::atomic::Ordering::SeqCst){return Err("app_stopping".into());}
        let tx=c.transaction().map_err(database_error)?;
        crate::memory::personal_state::generation::allow_dispatch(&tx,&input.run_id)?;
        let source=repo::source(&tx,&input.conversation_id,&input.run_id)?;
        if let Some(value)=repo::cached(&tx,&source,&call.id,&digest)?{return Ok(value);}
        let calls:i64=tx.query_row("SELECT COUNT(*) FROM coding_calls WHERE source_id=?1",[&source],|r|r.get(0)).map_err(database_error)?;
        if calls>=8{return Err("coding_call_budget_exhausted".into());}
        let settings=repo::settings(&tx)?;
        let value=match call.name.as_str(){
            "coding_start"=>{
                let args:Start=serde_json::from_value(canonical.clone()).map_err(|_|"invalid_arguments")?;
                if !settings.enabled{return Err("coding_disabled".into());}
                let existing:Option<String>=tx.query_row("SELECT id FROM coding_jobs WHERE source_id=?1",[&source],|r|r.get(0)).optional().map_err(database_error)?;
                if let Some(job)=existing{
                    let prior_digest:String=tx.query_row("SELECT digest FROM coding_runs WHERE job_id=?1 ORDER BY rowid LIMIT 1",[&job],|r|r.get(0)).map_err(database_error)?;
                    if prior_digest!=digest{return Err("idempotency_conflict".into());}
                    let mut value=repo::inspect(&tx,&input.conversation_id,&job,0,1)?;value["accepted"]=json!(true);value
                }else{
                    let path:String=tx.query_row("SELECT path FROM coding_workspaces WHERE id=?1 AND conversation_id=?2",params![args.workspace_id,input.conversation_id],|r|r.get(0)).map_err(|_|"workspace_required")?;
                    let actual=std::fs::canonicalize(&path).map_err(|_|"workspace_missing")?;
                    if actual.to_str()!=Some(path.as_str()) || !actual.join(".git").exists(){return Err("workspace_invalid".into());}
                    let job=new_id("coding");let run=new_id("coding_run");let directory=state.data_directory.join("coding-sessions");std::fs::create_dir_all(&directory).map_err(|_|"session_storage_unavailable")?;
                    let session=directory.join(format!("{job}.jsonl"));
                    tx.execute("INSERT INTO coding_jobs(id,conversation_id,source_id,workspace_id,workspace_path,settings_json,revision,session_path,state,current_run_id) VALUES(?1,?2,?3,?4,?5,?6,1,?7,'queued',?8)",params![job,input.conversation_id,source,args.workspace_id,path,serde_json::to_string(&settings).unwrap(),session.to_string_lossy(),run]).map_err(database_error)?;
                    insert_run(&tx,&job,&run,&source,&input.run_id,&args.request,&digest)?;
                    launch=Some(run.clone());json!({"jobId":job,"runId":run,"revision":1,"state":"queued","accepted":true})
                }
            },
            "coding_inspect"=>{let args:Inspect=serde_json::from_value(canonical.clone()).map_err(|_|"invalid_arguments")?;repo::inspect(&tx,&input.conversation_id,&args.job_id,args.cursor,args.limit)?},
            "coding_continue"=>{
                let args:Continue=serde_json::from_value(canonical.clone()).map_err(|_|"invalid_arguments")?;
                if !settings.enabled{return Err("coding_disabled".into());}
                repo::authorize(&tx,&args.job_id,&input.conversation_id)?;let (status,_)=repo::revision(&tx,&args.job_id,args.expected_revision)?;
                if !matches!(status.as_str(),"settled"|"failed"|"interrupted"){return Err("busy".into());}
                let run=new_id("coding_run");insert_run(&tx,&args.job_id,&run,&source,&input.run_id,&args.request,&digest)?;
                tx.execute("UPDATE coding_jobs SET revision=revision+1,state='queued',current_run_id=?2 WHERE id=?1",params![args.job_id,run]).map_err(database_error)?;
                launch=Some(run.clone());json!({"jobId":args.job_id,"runId":run,"revision":args.expected_revision+1,"state":"queued","accepted":true})
            },
            "coding_cancel"=>{let args:Cancel=serde_json::from_value(canonical.clone()).map_err(|_|"invalid_arguments")?;cancel(&tx,&input.conversation_id,&args.job_id,args.expected_revision,&args.reason)?},
            _=>return Err("invalid_tool".into()),
        };
        tx.execute("INSERT INTO coding_calls VALUES(?1,?2,?3,?4)",params![source,call.id,digest,value.to_string()]).map_err(database_error)?;
        tx.commit().map_err(database_error)?;Ok(value)
    })?;
    if let Some(run) = launch {
        let writer = Arc::clone(&state.sqlite_writer);
        std::thread::spawn(move || crate::runtime::pi::runner::run(writer, run));
    }
    Ok(result)
}
fn insert_run(
    c: &rusqlite::Connection,
    job: &str,
    run: &str,
    source: &str,
    host: &str,
    payload: &str,
    digest: &str,
) -> Result<(), String> {
    let busy:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM coding_runs WHERE state IN ('starting','running','stopping','outcome_unknown'))",[],|r|r.get(0)).map_err(database_error)?;
    if busy {
        return Err("busy".into());
    }
    c.execute("INSERT INTO coding_runs(id,job_id,source_id,host_run_id,payload,digest,delivery,state,started_at) VALUES(?1,?2,?3,?4,?5,?6,'prepared','starting',?7)",params![run,job,source,host,payload,digest,now_iso()]).map_err(database_error)?;
    repo::event(c, job, run, "queued", json!({}))
}
pub fn cancel(
    c: &rusqlite::Connection,
    conversation: &str,
    job: &str,
    revision: u64,
    reason: &str,
) -> Result<Value, String> {
    repo::authorize(c, job, conversation)?;
    let (status, run) = repo::revision(c, job, revision)?;
    if matches!(status.as_str(), "settled" | "failed" | "interrupted") {
        return Ok(json!({"jobId":job,"runId":run,"revision":revision,"state":status}));
    }
    if status == "outcome_unknown" {
        return Err("process_ownership_unresolved".into());
    }
    if status == "cancel_requested" {
        return Ok(
            json!({"jobId":job,"runId":run,"revision":revision,"state":status,"accepted":true}),
        );
    }
    c.execute("UPDATE coding_runs SET state='stopping',stop_reason=?2 WHERE id=?1 AND state!='outcome_unknown'",params![run,reason]).map_err(database_error)?;
    c.execute(
        "UPDATE coding_jobs SET state='cancel_requested',revision=revision+1 WHERE id=?1",
        [job],
    )
    .map_err(database_error)?;
    repo::event(c, job, &run, "cancel_requested", json!({}))?;
    Ok(
        json!({"jobId":job,"runId":run,"revision":revision+1,"state":"cancel_requested","accepted":true}),
    )
}
pub fn register(state: &AppState, conversation: &str, path: &str) -> Result<Value, String> {
    let canonical = std::fs::canonicalize(PathBuf::from(path)).map_err(|_| "workspace_missing")?;
    if !canonical.is_dir() || !canonical.join(".git").exists() {
        return Err("workspace_not_git".into());
    }
    let git = std::process::Command::new("git")
        .arg("-C")
        .arg(&canonical)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|_| "workspace_not_git")?;
    if !git.status.success()
        || std::fs::canonicalize(String::from_utf8_lossy(&git.stdout).trim())
            .ok()
            .as_ref()
            != Some(&canonical)
    {
        return Err("workspace_not_git".into());
    }
    state.sqlite_writer.write(|c|{let id=new_id("workspace");c.execute("INSERT INTO coding_workspaces(id,conversation_id,path) VALUES(?1,?2,?3) ON CONFLICT(conversation_id) DO UPDATE SET id=excluded.id,path=excluded.path",params![id,conversation,canonical.to_string_lossy()]).map_err(database_error)?;Ok(json!({"workspaceId":id,"path":canonical.to_string_lossy()}))})
}
