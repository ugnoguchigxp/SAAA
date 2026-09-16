//! Retryable remote stop and seven-phase absence; HTTP success is never sufficient.
use super::{managed::Adapter, product::outbox};
use crate::database_error;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
pub async fn cancel(a: &Adapter, id: &str) -> Result<(), String> {
    let subject = &a
        .product
        .as_ref()
        .ok_or("personal-product-unavailable")?
        .capability
        .subject_digest;
    let exists=a.writer.read_serialized(|c|c.query_row("SELECT EXISTS(SELECT 1 FROM personal_remote_operations WHERE id=?1 AND kind='attempt')",[id],|r|r.get::<_,bool>(0)).map_err(database_error))?;
    if !exists {
        return Ok(());
    }
    let result = a.client.cancel_attempt(id).await;
    match result {
        Ok(v)
            if v["attemptId"] == id
                && v["subjectDigest"] == *subject
                && v["stopState"] == "backend_stopped" =>
        {
            outbox::receipt(a, id, &v)?;
            a.writer.write(|c|{c.execute("UPDATE personal_generations SET cancellation='remote-stopped' WHERE attempt_id=?1",[id]).map_err(database_error)?;Ok(())})
        }
        _ => {
            a.writer.write(|c| {
                c.execute(
                    "UPDATE personal_remote_operations SET state='unknown' WHERE id=?1",
                    [id],
                )
                .map_err(database_error)?;
                Ok(())
            })?;
            Err("personal-remote-stop-pending".into())
        }
    }
}
pub async fn cleanup(a: &Adapter) -> Result<usize, String> {
    let cap = &a
        .product
        .as_ref()
        .ok_or("personal-product-unavailable")?
        .capability;
    // The shared slot prevents source cleanup invalidating a concurrent local View.
    let _slot = super::worker::foreground().await;
    let generations=a.writer.read_serialized(|c|{let mut q=c.prepare("SELECT DISTINCT o.generation_id FROM personal_remote_operations o JOIN personal_generations g ON g.id=o.generation_id WHERE o.subject=?1 AND g.status NOT IN ('prepared','running') AND o.state!='cleaned' ORDER BY o.created_at LIMIT 16").map_err(database_error)?;let rows=q.query_map([&cap.subject_digest],|r|r.get::<_,String>(0)).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;Ok(rows)})?;
    let mut completed = 0;
    for generation in generations {
        let (id,request)=a.writer.write(|c|{
            if let Some((id,raw))=c.query_row("SELECT id,receipt FROM personal_remote_operations WHERE generation_id=?1 AND kind='forget'",[&generation],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional().map_err(database_error)? {return Ok((id,super::decode::<Value>(raw)?));}
            let mut q=c.prepare("SELECT id,kind,receipt FROM personal_remote_operations WHERE generation_id=?1 AND kind IN ('source','attempt')").map_err(database_error)?;
            let rows=q.query_map([&generation],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;drop(q);
            let mut contexts=Vec::new();let mut handles=Vec::new();let mut attempts=Vec::new();
            for (id,kind,raw) in rows {if kind=="source"{contexts.push(id);let v:Value=super::decode(raw)?;if let Some(h)=v["sourceHandle"].as_str(){handles.push(h.to_string());}}else{attempts.push(id);}}
            // Even an attempt that never reached the server is tombstoned to reject late delivery.
            if attempts.is_empty(){let attempt:String=c.query_row("SELECT attempt_id FROM personal_generations WHERE id=?1",[&generation],|r|r.get(0)).map_err(database_error)?;attempts.push(attempt);}
            let id=crate::new_id("forget");
            let request=json!({"forgetId":id,"contextIds":contexts,"sourceHandles":handles,"attemptIds":attempts});
            c.execute("INSERT INTO personal_remote_operations(id,kind,generation_id,subject,allocation,runtime,receipt,created_at) VALUES(?1,'forget',?2,?3,?4,?5,?6,?7)",rusqlite::params![id,generation,cap.subject_digest,cap.allocation_id,cap.runtime,super::encode(&request)?,super::now()]).map_err(database_error)?;
            Ok((id,request))
        })?;
        // Each incarnation must also be forgotten when provision's response was lost.
        let sources = request["contextIds"]
            .as_array()
            .ok_or("personal-cleanup-targets")?;
        let mut all = true;
        for source in sources {
            let source = source.as_str().ok_or("personal-cleanup-targets")?;
            let source_forget = format!("forget-{source}");
            let r = json!({"forgetId":source_forget,"incarnation":source,"contextIds":[source],"attemptIds":request["attemptIds"],"sourceHandles":[]});
            // The deterministic ID and request fields are reconstructible from the durable parent.
            let v = match a.client.product_forget(&r).await {
                Ok(v) => v,
                Err(_) => {
                    all = false;
                    continue;
                }
            };
            super::product::diagnostics::record(a, &source_forget, &v)?;
            all &= saaa_larm_session::personal_state::forget_complete(
                &v,
                &cap.subject_digest,
                &source_forget,
            );
        }
        let v = match a.client.product_forget(&request).await {
            Ok(v) => v,
            Err(_) => {
                continue;
            }
        };
        super::product::diagnostics::record(a, &id, &v)?;
        all &= saaa_larm_session::personal_state::forget_complete(&v, &cap.subject_digest, &id);
        if all {
            let confirmed = a.client.product_get("forget", &id).await?;
            all = saaa_larm_session::personal_state::forget_complete(
                &confirmed,
                &cap.subject_digest,
                &id,
            );
        }
        a.writer.write(|c|{
            c.execute("UPDATE personal_cleanup SET cancel_sent=1,remote_stopped=?2,registration_absent=?2,source_absent=?2,snapshot_safe=?2,stage=CASE WHEN ?2 THEN 'complete' ELSE 'pending' END,last_code=CASE WHEN ?2 THEN 'confirmed' ELSE 'remote-cleanup-pending' END WHERE incarnation IN (SELECT id FROM personal_remote_operations WHERE generation_id=?1 AND kind='source')",rusqlite::params![generation,all]).map_err(database_error)?;
            if all {c.execute("UPDATE personal_remote_operations SET state='cleaned',request_digest='',receipt=CASE WHEN kind='forget' THEN receipt ELSE '{}' END WHERE generation_id=?1",[&generation]).map_err(database_error)?;c.execute("UPDATE personal_generations SET cancellation=CASE WHEN cancellation!='none' THEN 'remote-stopped' ELSE cancellation END WHERE id=?1",[&generation]).map_err(database_error)?;}
            Ok(())
        })?;
        if all {
            completed += 1;
        }
    }
    Ok(completed)
}
