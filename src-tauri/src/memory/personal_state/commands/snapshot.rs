//! Read-only continuity, World and maintenance snapshot projection.
use super::*;
pub fn snapshot(c: &Connection) -> Result<Value, String> {
    let ledger = super::super::store::load(c)?;
    let (pending,bytes,oldest):(u64,u64,Option<i64>)=c.query_row("SELECT count(*),COALESCE(sum(s.bytes),0),min(s.recorded_at) FROM personal_jobs j JOIN personal_sources s ON s.sequence=j.source_sequence WHERE j.status!='completed' AND s.available=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(database_error)?;
    let mut items = Vec::new();
    let mut world_items = Vec::new();
    for a in ledger.assertions.values() {
        let status = ledger.status(&a.id, super::super::now());
        let value: Option<String> = c
            .query_row(
                "SELECT value_json FROM personal_payloads WHERE id=?1",
                [&a.payload_ref],
                |r| r.get(0),
            )
            .ok();
        let item = json!({"id":a.id,"kind":a.kind,"key":a.semantic_key,"status":status,"source":a.evidence,"classification":a.access.classification,"scope":a.access.task_request,"value":value.and_then(|s|serde_json::from_str::<Value>(&s).ok())});
        if a.kind.is_world() {
            world_items.push(item);
        } else {
            items.push(item);
        }
    }
    let mut observations = c.prepare("SELECT o.id,o.scope_key,o.source_id,o.source_version,o.value_json FROM personal_world_observations o JOIN context_scopes s ON s.scope_key=o.scope_key AND s.state='active' WHERE NOT EXISTS (SELECT 1 FROM personal_world_observation_sources d LEFT JOIN personal_sources p ON p.message_id=d.source_id AND p.version=d.source_version AND p.available=1 LEFT JOIN personal_source_scope_refs r ON r.source_id=d.source_id AND r.version=d.source_version AND r.scope_key=o.scope_key WHERE d.observation_id=o.id AND (p.message_id IS NULL OR r.source_id IS NULL)) ORDER BY o.observed_at DESC,o.id LIMIT 128").map_err(database_error)?;
    for row in observations
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .map_err(database_error)?
    {
        let (id, scope, source, version, raw) = row.map_err(database_error)?;
        world_items.push(json!({"id":id,"kind":"world_observation","key":"world_observation","status":"observed","scope":scope,"source":[{"id":source,"version":version}],"value":serde_json::from_str::<Value>(&raw).map_err(|_|"world-projection-corrupt")?}));
    }
    let mut stmt=c.prepare("SELECT incarnation,stage,cancel_sent,remote_stopped,registration_absent,source_absent,snapshot_safe,last_code FROM personal_cleanup").map_err(database_error)?;
    let cleanup=stmt.query_map([],|r|Ok(json!({"incarnation":r.get::<_,String>(0)?,"stage":r.get::<_,String>(1)?,"cancelSent":r.get::<_,bool>(2)?,"remoteStopped":r.get::<_,bool>(3)?,"registrationAbsent":r.get::<_,bool>(4)?,"sourceAbsent":r.get::<_,bool>(5)?,"snapshotSafe":r.get::<_,bool>(6)?,"reason":r.get::<_,String>(7)?}))).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;
    let contract = super::super::contract::load(c).err();
    Ok(
        json!({"enabled":super::super::super::control_plane::memory_enabled(),"enabledOverride":std::env::var_os("SAAA_MEMORY_ENABLED").is_some(),"revision":ledger.revision,"inputEpoch":ledger.input_epoch,"policyRevision":ledger.policy_revision,"pendingCount":pending,"pendingBytes":bytes,"oldestPendingAt":oldest,"contractReady":contract.is_none(),"contractReason":contract,"items":items,"worldItems":world_items,"maintenance":super::super::maintenance::status(c)?,"cleanup":cleanup,"remoteCleanup":super::super::product::diagnostics::read(c)?,"physicalSecureEraseGuaranteed":false}),
    )
}
