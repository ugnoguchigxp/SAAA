//! One immutable request, one fresh View, one persisted generation attempt.
use super::{
    generation::{self, Manifest},
    managed::{iso, Adapter},
    registration::Pins,
    sources,
};
use crate::{database_error, RunCancellation};
use saaa_larm_session::contexts::{PlanItem, ViewRequest};
use saaa_personal_state_core::SourceRef;
use serde_json::Value;
use std::sync::Arc;

pub async fn infer(
    adapter: &Adapter,
    mut manifest: Manifest,
    request: &Value,
    required: &[SourceRef],
    cancel: Arc<RunCancellation>,
) -> Result<(Manifest, Value), String> {
    if adapter.product.is_some() {
        return super::product::infer(adapter, manifest, request, required, cancel).await;
    }
    if manifest.allocation != adapter.certification.allocation
        || manifest.runtime != adapter.certification.runtime
        || manifest.release != adapter.certification.release
        || manifest.lease_epoch != adapter.certification.lease_epoch
        || required
            .iter()
            .any(|source| !manifest.sources.contains(source))
    {
        return Err("personal-generation-binding".into());
    }
    adapter.writer.read_serialized(|c| {
        let used: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM personal_generations WHERE id=?1 OR attempt_id=?2)",
                rusqlite::params![manifest.generation_id, manifest.attempt_id],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        if used {
            return Err("personal-generation-replay".into());
        }
        Ok(())
    })?;
    let id = manifest.generation_id.clone();
    let result = tokio::select! {biased;
        _=cancel.cancelled()=>Err("personal-generation-cancelled".into()),
        result=tokio::time::timeout(std::time::Duration::from_secs(30),execute(adapter,&mut manifest,request,required,cancel.clone()))=>result.unwrap_or_else(|_|Err("personal-generation-timeout".into()))
    };
    match result {
        Ok(response) => Ok((manifest, response)),
        Err(error) => {
            adapter.writer.write(|c|{c.execute("UPDATE personal_generations SET output_allowed=0,status='failed',cancellation=CASE WHEN status='running' THEN 'sent-unconfirmed' ELSE cancellation END WHERE id=?1",[&id]).map_err(database_error)?;Ok(())})?;
            Err(error)
        }
    }
}
async fn execute(
    adapter: &Adapter,
    m: &mut Manifest,
    request: &Value,
    required: &[SourceRef],
    cancel: Arc<RunCancellation>,
) -> Result<Value, String> {
    if required.len() > 512 {
        return Err("personal-context-item-budget".into());
    }
    let mut pins = Pins::new(adapter);
    let base_tokens = adapter.delivery.measure(request, cancel.clone()).await?;
    let mut bytes = super::encode(request)?.len() as u64;
    adapter
        .certification
        .budget()
        .validate_materialized(base_tokens, bytes, adapter.certification.output_reserve)
        .map_err(|_| "personal-input-budget")?;
    let mut registrations = Vec::new();
    for source in required {
        let text = adapter.writer.read_serialized(|c| {
            sources::revalidate(c, source)?;
            let chunk = sources::load(
                c,
                source.sequence,
                source.key.start,
                (source.key.end - source.key.start).max(4) as usize,
            )?;
            Ok(chunk.text)
        })?;
        bytes = bytes
            .checked_add(text.len() as u64)
            .ok_or("personal-input-budget")?;
        if bytes > adapter.certification.max_bytes as u64 {
            return Err("personal-input-budget".into());
        }
        registrations.push(pins.register(source, &text, cancel.clone()).await?);
    }
    if registrations.iter().map(|r| r.token_count).sum::<u64>() > 20_000_000 {
        return Err("personal-source-quota".into());
    }
    let view = if registrations.is_empty() {
        None
    } else {
        let plan = ViewRequest {
            allocation_id: adapter.certification.allocation.clone(),
            runtime: adapter.certification.runtime.clone(),
            base_input_tokens: base_tokens,
            max_input_tokens: adapter.certification.max_input_tokens,
            deadline: iso(super::now() + 30000)?,
            canonicalization_version: "context-view-v1".into(),
            items: registrations
                .iter()
                .map(|r| PlanItem {
                    context_id: r.id.clone(),
                    version: r.version.clone(),
                    required: true,
                    utility: 1.0,
                })
                .collect(),
        };
        let view = adapter
            .client
            .create_view(&plan, &crate::new_id("view-request"))
            .await?;
        view.validate(
            &plan,
            &adapter.certification.release,
            adapter.certification.lease_epoch,
            super::now(),
        )?;
        for item in &view.ordered_items {
            if !registrations.iter().any(|r| {
                r.id == item.context_id
                    && r.version == item.version
                    && r.source_digest == item.source_digest
            }) {
                return Err("personal-view-source-digest".into());
            }
        }
        m.view_id = Some(view.id.clone());
        m.view_digest = Some(view.view_digest.clone());
        m.expires_at = chrono::DateTime::parse_from_rfc3339(&view.expires_at)
            .map_err(|_| "personal-view-expiry")?
            .timestamp_millis()
            .min(m.expires_at);
        Some(view)
    };
    m.request_digest = super::generation::request_digest(request)?;
    adapter.writer.write(|c| {
        let tx = c.transaction().map_err(database_error)?;
        generation::allow_run(&tx, &m.run_id)?;
        generation::prepare(&tx, m)?;
        generation::dispatch(&tx, &m.generation_id)?;
        tx.commit().map_err(database_error)
    })?;
    let response = if let Some(view) = &view {
        let response = adapter
            .client
            .chat(view, &adapter.certification.capability, request)
            .await?;
        if adapter.client.operation(&view.operation_id).await?["state"] != "succeeded" {
            return Err("personal-materialization-unconfirmed".into());
        }
        response
    } else {
        adapter
            .client
            .base_chat(
                &adapter.certification.allocation,
                &adapter.certification.capability,
                request,
            )
            .await?
    };
    adapter.writer.write(|c| {
        generation::allow(c, &m.generation_id)?;
        let success = matches!(
            response["choices"][0]["finish_reason"].as_str(),
            Some("stop" | "tool_calls")
        );
        generation::finish(c, &m.generation_id, success)?;
        if view.is_some() {
            c.execute(
                "UPDATE personal_generations SET materialization='succeeded' WHERE id=?1",
                [&m.generation_id],
            )
            .map_err(database_error)?;
        }
        if !success {
            return Err("personal-generation-incomplete".into());
        }
        Ok(())
    })?;
    Ok(response)
}
