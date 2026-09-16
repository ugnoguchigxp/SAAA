use crate::{database_error, AppState};
use rusqlite::Connection;
use serde_json::{json, Value};
use tauri::Emitter;

pub fn snapshot(c: &Connection) -> Result<Value, String> {
    let ledger = super::store::load(c)?;
    let (pending,bytes,oldest):(u64,u64,Option<i64>)=c.query_row("SELECT count(*),COALESCE(sum(s.bytes),0),min(s.recorded_at) FROM personal_jobs j JOIN personal_sources s ON s.sequence=j.source_sequence WHERE j.status!='completed' AND s.available=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(database_error)?;
    let mut items = Vec::new();
    for a in ledger.assertions.values() {
        let status = ledger.status(&a.id, super::now());
        let value: Option<String> = c
            .query_row(
                "SELECT value_json FROM personal_payloads WHERE id=?1",
                [&a.payload_ref],
                |r| r.get(0),
            )
            .ok();
        items.push(json!({"id":a.id,"kind":a.kind,"key":a.semantic_key,"status":status,"source":a.evidence,"classification":a.access.classification,"value":value.and_then(|s|serde_json::from_str::<Value>(&s).ok())}));
    }
    let mut stmt=c.prepare("SELECT incarnation,stage,cancel_sent,remote_stopped,registration_absent,source_absent,snapshot_safe,last_code FROM personal_cleanup").map_err(database_error)?;
    let cleanup=stmt.query_map([],|r|Ok(json!({"incarnation":r.get::<_,String>(0)?,"stage":r.get::<_,String>(1)?,"cancelSent":r.get::<_,bool>(2)?,"remoteStopped":r.get::<_,bool>(3)?,"registrationAbsent":r.get::<_,bool>(4)?,"sourceAbsent":r.get::<_,bool>(5)?,"snapshotSafe":r.get::<_,bool>(6)?,"reason":r.get::<_,String>(7)?}))).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;
    let contract = super::contract::load(c).err();
    Ok(
        json!({"enabled":super::super::control_plane::memory_enabled(),"revision":ledger.revision,"inputEpoch":ledger.input_epoch,"policyRevision":ledger.policy_revision,"pendingCount":pending,"pendingBytes":bytes,"oldestPendingAt":oldest,"contractReady":contract.is_none(),"contractReason":contract,"items":items,"cleanup":cleanup,"remoteCleanup":super::product::diagnostics::read(c)?,"physicalSecureEraseGuaranteed":false}),
    )
}
#[tauri::command]
pub fn personal_state_snapshot(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    state.sqlite_writer.read_serialized(snapshot)
}
#[tauri::command]
pub fn forget_personal_source(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    source_id: String,
) -> Result<Value, String> {
    crate::validate_identifier(&source_id, "source id")?;
    let runs=state.sqlite_writer.write(|c|{
        let tx=c.transaction().map_err(database_error)?;
        let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM personal_sources WHERE message_id=?1)",[&source_id],|r|r.get(0)).map_err(database_error)?;
        if !exists{return Err("personal-source-unavailable".into());}
        tx.execute("DELETE FROM conversation_messages WHERE id=?1",[&source_id]).map_err(database_error)?;
        let mut stmt=tx.prepare("SELECT DISTINCT run_id FROM personal_generations WHERE output_allowed=0 AND cancellation='requested'").map_err(database_error)?;
        let runs=stmt.query_map([],|r|r.get::<_,String>(0)).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;
        drop(stmt);super::store::rebuild(&tx,super::now())?;tx.commit().map_err(database_error)?;Ok(runs)
    })?;
    super::worker::interrupt();
    for run in &runs {
        if let Some(cancel) = state
            .active_runs
            .lock()
            .map_err(|_| "personal-cancel-unavailable")?
            .get(run)
        {
            cancel.cancel();
        }
        state.streaming_tts.cancel(run);
    }
    state.sqlite_writer.write(|c|{for run in &runs {c.execute("UPDATE personal_generations SET cancellation='sent-unconfirmed' WHERE run_id=?1 AND output_allowed=0",[run]).map_err(database_error)?;}Ok(())})?;
    app.emit(
        "personal-state-forgotten",
        json!({"revisionChanged":true,"runIds":runs}),
    )
    .map_err(|_| "personal-forget-ui-notify")?;
    state.sqlite_writer.read_serialized(snapshot)
}
#[tauri::command]
pub async fn personal_state_extract_once(
    state: tauri::State<'_, AppState>,
) -> Result<Value, String> {
    // Certification is checked before claim. No failed jobs are manufactured just
    // because the host deployment has not supplied a delivery/measurement adapter.
    let adapter = super::managed::Adapter::configured(state.sqlite_writer.clone()).await?;
    let worked = super::worker::tick(
        &state.sqlite_writer,
        &adapter,
        super::super::control_plane::memory_enabled(),
    )
    .await?;
    adapter.cleanup().await?;
    Ok(json!({"worked":worked}))
}
/// Exposes progress without source text in application-wide diagnostics.
pub fn summary(c: &Connection) -> Result<Value, String> {
    let (revision, epoch): (u64, u64) = c
        .query_row("SELECT revision,input_epoch FROM personal_scope", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .map_err(database_error)?;
    let pending: u64 = c
        .query_row(
            "SELECT count(*) FROM personal_jobs WHERE status!='completed'",
            [],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    Ok(json!({"revision":revision,"inputEpoch":epoch,"pendingJobs":pending,"ready":false}))
}

#[tauri::command]
pub fn personal_source_page(
    state: tauri::State<'_, AppState>,
    after_sequence: u64,
) -> Result<Value, String> {
    state.sqlite_writer.read_serialized(|c|{
        let mut sources=Vec::new();let mut next=after_sequence;
        for seq in super::sources::page(c,after_sequence,32)? {
            let chunk=super::sources::load(c,seq,0,256)?;
            sources.push(json!({"id":chunk.source.key.id,"version":chunk.source.key.version,"sequence":seq,"preview":chunk.text,"bytes":chunk.total_bytes}));next=seq;
        }
        let more=!super::sources::page(c,next,1)?.is_empty();
        Ok(json!({"sources":sources,"nextSequence":next,"hasMore":more}))
    })
}
