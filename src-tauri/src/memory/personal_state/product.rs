//! Product HTTP orchestration; synthetic v1 adapters are not a production fallback.
use super::{
    generation::{self, Manifest},
    managed::Adapter,
    sources,
};
use crate::{database_error, RunCancellation};
use saaa_larm_session::{contexts::Registration, personal_state::Capability};
use saaa_personal_state_core::SourceRef;
use serde_json::{json, Value};
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
    let purpose = if m.purpose == "personal_state_extract" {
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
    let mut registrations = Vec::new();
    let mut total_tokens = 0u64;
    let mut total_bytes = 0u64;
    for s in required {
        let text = a.writer.read_serialized(|c| {
            sources::revalidate(c, s)?;
            Ok(sources::load(
                c,
                s.sequence,
                s.key.start,
                (s.key.end - s.key.start).max(4) as usize,
            )?
            .text)
        })?;
        let id = crate::new_id("source");
        remember(a, &id, "source", m, &s.digest)?;
        a.writer.write(|c|{generation::allow(c,&m.generation_id)?;c.execute("INSERT INTO personal_registrations VALUES(?1,?2,?3,?1,'{}','active','provisioning',1,?4)",rusqlite::params![id,s.key.id,s.key.version,m.expires_at]).map_err(database_error)?;c.execute("INSERT INTO personal_cleanup(incarnation,source_id) VALUES(?1,?2)",rusqlite::params![id,s.key.id]).map_err(database_error)?;Ok(())})?;
        let p = match a.client.provision_source(cap, &id, &s.digest, &text).await {
            Ok(p) => p,
            Err(_) => a.client.product_get("source", &id).await?,
        };
        cap.receipt(&p)?;
        if p["incarnation"] != id
            || p["sourceDigest"] != s.digest
            || p["byteCount"] != text.len() as u64
            || p["state"] != "succeeded"
        {
            return Err("personal-provision-attestation".into());
        }
        let handle = p["sourceHandle"].as_str().ok_or("personal-source-handle")?;
        saaa_larm_session::personal_state::identifier(handle)?;
        let tokens = p["tokenCount"].as_u64().ok_or("personal-source-tokens")?;
        total_tokens = total_tokens
            .checked_add(tokens)
            .ok_or("personal-source-quota")?;
        total_bytes = total_bytes
            .checked_add(text.len() as u64)
            .ok_or("personal-source-quota")?;
        if total_tokens > cap.source_token_limit || total_bytes > cap.max_total_source_bytes {
            return Err("personal-source-quota".into());
        }
        receipt(a, &id, &p)?;
        let r = Registration {
            id: id.clone(),
            version: s.key.version.to_string(),
            source_handle: handle.into(),
            source_digest: s.digest.clone(),
            classification: "confidential".into(),
            byte_count: text.len() as u64,
            token_count: tokens,
            tokenizer_digest: cap.tokenizer_digest.clone(),
            expires_at: Some(super::managed::iso(m.expires_at)?),
        };
        a.writer.write(|c|{generation::allow(c,&m.generation_id)?;sources::revalidate(c,s)?;c.execute("UPDATE personal_registrations SET metadata=?2,observed='registering' WHERE incarnation=?1 AND desired='active'",rusqlite::params![id,super::encode(&r)?]).map_err(database_error)?;Ok(())})?;
        let expected = serde_json::to_value(&r).map_err(|_| "personal-registration-schema")?;
        let descriptor = a.client.register(&r, &id).await?;
        if descriptor["state"] != "active"
            || expected
                .as_object()
                .ok_or("personal-registration-schema")?
                .iter()
                .any(|(k, v)| descriptor.get(k) != Some(v))
        {
            return Err("personal-registration-binding".into());
        }
        registrations.push(r);
    }
    let measure_id = crate::new_id("measure");
    remember(a, &measure_id, "measurement", m, &m.request_digest)?;
    let measured = match a.client.measure_request(cap, &measure_id, request).await {
        Ok(v) => v,
        Err(_) => a.client.product_get("measurement", &measure_id).await?,
    };
    cap.receipt(&measured)?;
    // LARM's canonical digest is authoritative; it also binds the full request at consume.
    let request_digest = measured["requestDigest"]
        .as_str()
        .filter(|s| s.len() == 64)
        .ok_or("personal-measurement-digest")?;
    if measured["measurementId"] != measure_id
        || measured["maxInputTokens"] != cap.input_limit()?
        || measured["baseInputTokens"]
            .as_u64()
            .is_none_or(|n| n > cap.input_limit().unwrap_or(0))
    {
        return Err("personal-measurement-binding".into());
    }
    receipt(a, &measure_id, &measured)?;
    if !registrations.is_empty() {
        let id = crate::new_id("view");
        remember(a, &id, "view", m, &m.request_digest)?;
        let plan = json!({"viewRequestId":id,"measurementId":measure_id,"allocationId":cap.allocation_id,"runtime":cap.runtime,"maxInputTokens":cap.input_limit()?,"deadline":super::managed::iso(m.expires_at.min(super::now()+30000))?,"canonicalizationVersion":"context-view-v2","request":request,"items":registrations.iter().map(|r|json!({"contextId":r.id,"version":r.version,"required":true,"utility":1})).collect::<Vec<_>>()});
        let view = match a.client.product_view(&plan, &id).await {
            Ok(v) => v,
            Err(e) => {
                let _ = a.client.product_get("view", &id).await;
                return Err(e);
            }
        };
        if view.schema_version != 1
            || view.allocation_id != cap.allocation_id
            || view.runtime != cap.runtime
            || view.release != cap.release
            || view.lease_epoch != cap.lease_epoch
            || view.canonicalization_version != "context-view-v2"
            || view.request_digest.as_deref() != Some(request_digest)
            || view.data_epoch != measured["dataEpoch"].as_u64()
            || view.state != "ready"
            || view.token_count > cap.input_limit()?
            || view.input_budget_tokens > cap.input_limit()?
            || !view.omitted.is_empty()
            || view.ordered_items.len() != registrations.len()
            || view.ordered_items.iter().any(|item| {
                !item.required
                    || !registrations.iter().any(|r| {
                        r.id == item.context_id
                            && r.version == item.version
                            && r.source_digest == item.source_digest
                    })
            })
        {
            return Err("personal-view-binding".into());
        }
        let v = a.client.product_get("view", &id).await?;
        if v["subjectDigest"] != cap.subject_digest
            || v["bootEpoch"] != cap.boot_epoch
            || v["viewId"] != view.id
            || v["requestDigest"] != request_digest
            || v["state"] != "ready"
        {
            return Err("personal-view-receipt".into());
        }
        receipt(a, &id, &v)?;
        m.expires_at = m.expires_at.min(saaa_larm_session::personal_state::instant(
            &view.expires_at,
        )?);
        m.view_id = Some(view.id);
        m.view_digest = Some(view.view_digest);
    }
    remember(a, &m.attempt_id, "attempt", m, request_digest)?;
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
        || attempt["requestDigest"] != request_digest
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
