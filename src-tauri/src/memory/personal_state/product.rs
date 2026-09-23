//! Product HTTP orchestration; synthetic v1 adapters are not a production fallback.
pub(super) use super::{
    generation::{self, Manifest},
    managed::Adapter,
};
pub(super) use crate::persistence::SqliteWriter;
use crate::{database_error, RunCancellation};
pub(super) use rusqlite::Connection;
use saaa_larm_session::personal_state::Capability;
use saaa_personal_state_core::SourceRef;
pub(super) use serde_json::{json, Value};
use std::sync::Arc;
pub struct Product {
    pub can_generate: bool,
    pub capability: Capability,
    pub _lease: Option<saaa_larm_session::Use>,
}

pub(super) mod diagnostics;
pub(super) mod outbox;
use outbox::{receipt, remember, Flight};
pub async fn infer(
    a: &Adapter,
    mut m: Manifest,
    request: &Value,
    required: &[SourceRef],
    cancel: Arc<RunCancellation>,
) -> Result<(Manifest, Value), String> {
    let cap = &a
        .product
        .as_ref()
        .ok_or("personal-product-unavailable")?
        .capability;
    if !a.product.as_ref().is_some_and(|p| p.can_generate) {
        return Err("personal-capability-unavailable".into());
    }
    cap.validate(&cap.subject_digest, &m.allocation, &m.runtime, super::now())?;
    let output_limit = cap.output_limit()?;
    if m.release != cap.release
        || m.lease_epoch != cap.lease_epoch
        || required.len() > 512
        || required.iter().any(|s| !m.sources.contains(s))
        || request["model"] != a.certification.model
        || request["stream"] != false
        || request["max_tokens"]
            .as_u64()
            .is_none_or(|n| n == 0 || n > output_limit)
    {
        return Err("personal-generation-binding".into());
    }
    let purpose = if matches!(
        m.purpose.as_str(),
        "personal_state_extract" | "world-extraction"
    ) {
        saaa_personal_state_core::Purpose::StateExtract
    } else {
        saaa_personal_state_core::Purpose::Reasoning
    };
    for source in &m.sources {
        if !source
            .access
            .permits(&saaa_personal_state_core::AccessRequest {
                principal: &a.certification.principal,
                scope: "primary",
                task_request: source.access.task_request.as_deref(),
                purpose,
                max_classification: saaa_personal_state_core::Classification::Confidential,
                policy_revision: m.policy_revision,
                authorized: true,
            })
        {
            return Err("personal-source-unauthorized".into());
        }
    }
    if super::encode(request)?.len() as u64 > cap.max_materialized_bytes {
        return Err("personal-input-byte-budget".into());
    }
    m.request_digest = generation::request_digest(request)?;
    a.writer.write(|c|{let tx=c.transaction().map_err(database_error)?;generation::allow_run(&tx,&m.run_id)?;
        let uncertain:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM personal_remote_operations o JOIN personal_generations g ON g.id=o.generation_id WHERE (o.kind='attempt' AND o.state IN ('pending','dispatched','unknown')) OR (o.state!='cleaned' AND (o.kind='forget' OR g.output_allowed=0)))",[],|r|r.get(0)).map_err(database_error)?;
        if uncertain{return Err("personal-remote-stop-pending".into());}
        generation::prepare(&tx,&m)?;tx.commit().map_err(database_error)})?;
    let _flight = Flight {
        writer: a.writer.clone(),
        generation: m.generation_id.clone(),
    };
    let result = tokio::select! {biased;
        _=cancel.cancelled()=>Err("personal-generation-cancelled".into()),
        v=tokio::time::timeout(std::time::Duration::from_secs(30),execute(a,&mut m,request,required,cancel.clone()))=>v.unwrap_or_else(|_|Err("personal-generation-timeout".into()))
    };
    // Cleanup ownership is durable, including drops before receiving a provision handle.
    a.writer.write(|c|{c.execute("UPDATE personal_registrations SET pins=0,desired='deleted' WHERE incarnation IN (SELECT id FROM personal_remote_operations WHERE generation_id=?1 AND kind='source')",[&m.generation_id]).map_err(database_error)?;Ok(())})?;
    match result {
        Ok(v) => Ok((m, v)),
        Err(error) => {
            a.writer.write(|c|{c.execute("UPDATE personal_generations SET output_allowed=0,status='failed',cancellation='requested' WHERE id=?1",[&m.generation_id]).map_err(database_error)?;Ok(())})?;
            // Request cancellation outside the cancelled request future; pending state survives failure.
            let _ = super::product_cleanup::cancel(a, &m.attempt_id).await;
            Err(error)
        }
    }
}
async fn execute(
    a: &Adapter,
    m: &mut Manifest,
    request: &Value,
    required: &[SourceRef],
    cancel: Arc<RunCancellation>,
) -> Result<Value, String> {
    let cap = &a
        .product
        .as_ref()
        .ok_or("personal-product-unavailable")?
        .capability;
    let materialized = super::materializer::prepare(a, m, request, required).await?;
    remember(a, &m.attempt_id, "attempt", m, &materialized.request_digest)?;
    a.writer.write(|c| {
        generation::allow(c, &m.generation_id)?;
        c.execute(
            "UPDATE personal_generations SET manifest_json=?2,view_id=?3 WHERE id=?1",
            rusqlite::params![m.generation_id, super::encode(m)?, m.view_id],
        )
        .map_err(database_error)?;
        generation::dispatch(c, &m.generation_id)
    })?;
    cancel.with_active(|| {
        a.writer.write(|c| {
            generation::allow(c, &m.generation_id)?;
            c.execute(
                "UPDATE personal_remote_operations SET state='dispatched' WHERE id=?1",
                [&m.attempt_id],
            )
            .map_err(database_error)?;
            Ok(())
        })
    })?;
    let response = a
        .client
        .product_chat(cap, &m.attempt_id, m.view_id.as_deref(), request)
        .await?;
    let attempt = a.client.product_get("attempt", &m.attempt_id).await?;
    if attempt["contractVersion"] != saaa_larm_session::personal_state::VERSION
        || attempt["subjectDigest"] != cap.subject_digest
        || attempt["allocationId"] != cap.allocation_id
        || attempt["runtime"] != cap.runtime
        || attempt["release"] != cap.release
        || attempt["attemptId"] != m.attempt_id
        || attempt["requestDigest"] != materialized.request_digest
        || attempt["state"] != "completed"
    {
        return Err("personal-attempt-unconfirmed".into());
    }
    receipt(a, &m.attempt_id, &attempt)?;
    a.writer.write(|c| {
        generation::allow(c, &m.generation_id)?;
        let success = matches!(
            response["choices"][0]["finish_reason"].as_str(),
            Some("stop" | "tool_calls")
        );
        generation::finish(c, &m.generation_id, success)?;
        if !success {
            return Err("personal-generation-incomplete".into());
        }
        c.execute(
            "UPDATE personal_generations SET materialization=?2 WHERE id=?1",
            rusqlite::params![
                m.generation_id,
                if m.view_id.is_some() {
                    "succeeded"
                } else {
                    "not-used"
                }
            ],
        )
        .map_err(database_error)?;
        Ok(())
    })?;
    Ok(response)
}
