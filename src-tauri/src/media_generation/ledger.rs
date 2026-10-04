//! Durable submission identity. No prompt or API key is stored here.
use super::*;
use crate::providers::service_registry::ResolvedRoute;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

pub(super) fn initialize(db: &rusqlite::Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS purpose_media_operations(
        run_id TEXT PRIMARY KEY,kind TEXT NOT NULL,route_json TEXT NOT NULL,
        state TEXT NOT NULL,remote_id TEXT,result_json TEXT,error_json TEXT,updated_at TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS purpose_media_artifacts(
        run_id TEXT NOT NULL,artifact_index INTEGER NOT NULL,content BLOB NOT NULL,
        PRIMARY KEY(run_id,artifact_index),FOREIGN KEY(run_id) REFERENCES purpose_media_operations(run_id));").map_err(crate::database_error)
}

pub(super) fn reserve(
    state: &AppState,
    run: &str,
    kind: MediaKind,
    route: &ResolvedRoute,
) -> Result<(), String> {
    state.sqlite_writer.write(|db| {
        initialize(db)?;
        let tx=db.transaction().map_err(crate::database_error)?;
        crate::providers::service_registry::validate_active(&tx,route)?;
        let inserted=tx.execute("INSERT OR IGNORE INTO purpose_media_operations(run_id,kind,route_json,state,updated_at) VALUES(?1,?2,?3,'reserved',?4)",params![run,serde_json::to_string(&kind).map_err(|e|e.to_string())?,serde_json::to_string(route).map_err(|e|e.to_string())?,crate::now_iso()]).map_err(crate::database_error)?;
        if inserted!=1 {return Err("この生成要求は記録済みです。再送せず、進行状況を確認してください。".into());}
        tx.commit().map_err(crate::database_error)
    })
}

pub(super) fn phase(
    state: &AppState,
    run: &str,
    phase: &str,
    job: Option<&str>,
) -> Result<(), String> {
    state.sqlite_writer.write(|db| {
        let count=db.execute("UPDATE purpose_media_operations SET state=?2,remote_id=COALESCE(?3,remote_id),updated_at=?4 WHERE run_id=?1 AND state NOT IN ('accepted','cancelled')",params![run,phase,job,crate::now_iso()]).map_err(crate::database_error)?;
        if count!=1{return Err("生成の記録を更新できません".into());}Ok(())
    })
}

pub(super) fn finish(
    state: &AppState,
    run: &str,
    route: &ResolvedRoute,
    result: &Result<MediaResult, MediaError>,
) -> Result<(), String> {
    state.sqlite_writer.write(|db| {
        let tx=db.transaction().map_err(crate::database_error)?;
        let (status,result_json,error_json,job)=match result {
            Ok(result)=>{
                let current:String=tx.query_row("SELECT state FROM purpose_media_operations WHERE run_id=?1",[run],|r|r.get(0)).map_err(crate::database_error)?;
                if current=="cancel_requested"||current=="cancelled" {return Err("中止要求済みの生成結果は採用できません".into());}
                crate::providers::service_registry::validate_active(&tx,route)?;
                let mut used=route.clone(); used.model=result.model.clone();
                crate::providers::service_registry::operations::accepted(&tx,run,&used)?;
                ("accepted",Some(serde_json::to_string(result).map_err(|e|e.to_string())?),None,result.job_id.as_deref())
            },
            Err(error)=>(if error.kind==saaa_larm_session::media::FailureKind::Cancelled && !error.may_have_generated {"cancelled"}else if error.may_have_generated {"unknown"}else{"failed"},None,Some(serde_json::to_string(error).map_err(|e|e.to_string())?),error.job_id.as_deref()),
        };
        let count=tx.execute("UPDATE purpose_media_operations SET state=?2,result_json=?3,error_json=?4,remote_id=COALESCE(?5,remote_id),updated_at=?6 WHERE run_id=?1",params![run,status,result_json,error_json,job,crate::now_iso()]).map_err(crate::database_error)?;
        if count!=1{return Err("生成結果を記録できません".into());}tx.commit().map_err(crate::database_error)
    })
}

pub(super) fn get(state: &AppState, run: &str) -> Result<Option<Value>, String> {
    // Initialize via the one writer before reading on another connection.
    state.sqlite_writer.write(|db| initialize(db))?;
    state.sqlite_readers.read(|db| {
        db.query_row("SELECT kind,route_json,state,remote_id,result_json,error_json,updated_at FROM purpose_media_operations WHERE run_id=?1",[run],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,String>(6)?)))
            .optional().map_err(crate::database_error)?.map(|(kind,route,status,job,result,error,at)|{
                Ok(json!({"runId":run,"kind":serde_json::from_str::<Value>(&kind).map_err(|e|e.to_string())?,"route":serde_json::from_str::<Value>(&route).map_err(|e|e.to_string())?,"status":status,"jobId":job,"result":result.map(|v|serde_json::from_str::<Value>(&v)).transpose().map_err(|e|e.to_string())?,"error":error.map(|v|serde_json::from_str::<Value>(&v)).transpose().map_err(|e|e.to_string())?,"updatedAt":at}))
            }).transpose()
    })
}

pub(super) fn cached(state: &AppState, run: &str, index: usize) -> Result<Option<Vec<u8>>, String> {
    state.sqlite_writer.write(|db| initialize(db))?;
    state.sqlite_readers.read(|db|db.query_row("SELECT a.content FROM purpose_media_artifacts a JOIN purpose_media_operations o ON o.run_id=a.run_id WHERE a.run_id=?1 AND a.artifact_index=?2 AND o.state='accepted'",params![run,index],|r|r.get(0)).optional().map_err(crate::database_error))
}

pub(super) fn cache(state: &AppState, run: &str, index: usize, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > 64 * 1024 * 1024 {
        return Err("成果物が大きすぎます".into());
    }
    state.sqlite_writer.write(|db| {
        db.execute("INSERT OR IGNORE INTO purpose_media_artifacts(run_id,artifact_index,content) VALUES(?1,?2,?3)",params![run,index,bytes]).map_err(crate::database_error)?;Ok(())
    })
}
