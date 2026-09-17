//! LARM source/View preparation. This module never starts model inference.
use super::{
    generation::{self, Manifest},
    managed::Adapter,
    product::outbox::{receipt, remember},
    sources,
};
use crate::database_error;
use saaa_larm_session::contexts::Registration;
use saaa_personal_state_core::SourceRef;
use serde_json::{json, Value};

pub(super) struct Prepared {
    pub(super) request_digest: String,
}

pub(super) async fn prepare(
    adapter: &Adapter,
    manifest: &mut Manifest,
    request: &Value,
    required: &[SourceRef],
) -> Result<Prepared, String> {
    let capability = &adapter
        .product
        .as_ref()
        .ok_or("personal-product-unavailable")?
        .capability;
    let mut registrations = Vec::new();
    let mut total_tokens = 0u64;
    let mut total_bytes = 0u64;
    for source in required {
        let text = adapter.writer.read_serialized(|connection| {
            sources::revalidate(connection, source)?;
            Ok(sources::load(
                connection,
                source.sequence,
                source.key.start,
                (source.key.end - source.key.start).max(4) as usize,
            )?
            .text)
        })?;
        let id = crate::new_id("source");
        remember(adapter, &id, "source", manifest, &source.digest)?;
        adapter.writer.write(|connection| {
            generation::allow(connection, &manifest.generation_id)?;
            connection.execute(
                "INSERT INTO personal_registrations VALUES(?1,?2,?3,?1,'{}','active','provisioning',1,?4)",
                rusqlite::params![id, source.key.id, source.key.version, manifest.expires_at],
            ).map_err(database_error)?;
            connection.execute(
                "INSERT INTO personal_cleanup(incarnation,source_id) VALUES(?1,?2)",
                rusqlite::params![id, source.key.id],
            ).map_err(database_error)?;
            Ok(())
        })?;
        let provision = match adapter
            .client
            .provision_source(capability, &id, &source.digest, &text)
            .await
        {
            Ok(provision) => provision,
            Err(_) => adapter.client.product_get("source", &id).await?,
        };
        capability.receipt(&provision)?;
        if provision["incarnation"] != id
            || provision["sourceDigest"] != source.digest
            || provision["byteCount"] != text.len() as u64
            || provision["state"] != "succeeded"
        {
            return Err("personal-provision-attestation".into());
        }
        let handle = provision["sourceHandle"]
            .as_str()
            .ok_or("personal-source-handle")?;
        saaa_larm_session::personal_state::identifier(handle)?;
        let tokens = provision["tokenCount"]
            .as_u64()
            .ok_or("personal-source-tokens")?;
        total_tokens = total_tokens
            .checked_add(tokens)
            .ok_or("personal-source-quota")?;
        total_bytes = total_bytes
            .checked_add(text.len() as u64)
            .ok_or("personal-source-quota")?;
        if total_tokens > capability.source_token_limit
            || total_bytes > capability.max_total_source_bytes
        {
            return Err("personal-source-quota".into());
        }
        receipt(adapter, &id, &provision)?;
        let registration = Registration {
            id: id.clone(),
            version: source.key.version.to_string(),
            source_handle: handle.into(),
            source_digest: source.digest.clone(),
            classification: "confidential".into(),
            byte_count: text.len() as u64,
            token_count: tokens,
            tokenizer_digest: capability.tokenizer_digest.clone(),
            expires_at: Some(super::managed::iso(manifest.expires_at)?),
        };
        adapter.writer.write(|connection| {
            generation::allow(connection, &manifest.generation_id)?;
            sources::revalidate(connection, source)?;
            connection.execute(
                "UPDATE personal_registrations SET metadata=?2,observed='registering' WHERE incarnation=?1 AND desired='active'",
                rusqlite::params![id, super::encode(&registration)?],
            ).map_err(database_error)?;
            Ok(())
        })?;
        let expected =
            serde_json::to_value(&registration).map_err(|_| "personal-registration-schema")?;
        let descriptor = adapter.client.register(&registration, &id).await?;
        if descriptor["state"] != "active"
            || expected
                .as_object()
                .ok_or("personal-registration-schema")?
                .iter()
                .any(|(key, value)| descriptor.get(key) != Some(value))
        {
            return Err("personal-registration-binding".into());
        }
        registrations.push(registration);
    }
    let measurement_id = crate::new_id("measure");
    remember(
        adapter,
        &measurement_id,
        "measurement",
        manifest,
        &manifest.request_digest,
    )?;
    let measured = match adapter
        .client
        .measure_request(capability, &measurement_id, request)
        .await
    {
        Ok(value) => value,
        Err(_) => {
            adapter
                .client
                .product_get("measurement", &measurement_id)
                .await?
        }
    };
    capability.receipt(&measured)?;
    let request_digest = measured["requestDigest"]
        .as_str()
        .filter(|value| value.len() == 64)
        .ok_or("personal-measurement-digest")?
        .to_string();
    if measured["measurementId"] != measurement_id
        || measured["maxInputTokens"] != capability.input_limit()?
        || measured["baseInputTokens"]
            .as_u64()
            .is_none_or(|tokens| tokens > capability.input_limit().unwrap_or(0))
    {
        return Err("personal-measurement-binding".into());
    }
    receipt(adapter, &measurement_id, &measured)?;
    if !registrations.is_empty() {
        let id = crate::new_id("view");
        remember(adapter, &id, "view", manifest, &manifest.request_digest)?;
        let plan = json!({
            "viewRequestId": id,
            "measurementId": measurement_id,
            "allocationId": capability.allocation_id,
            "runtime": capability.runtime,
            "maxInputTokens": capability.input_limit()?,
            "deadline": super::managed::iso(manifest.expires_at.min(super::now() + 30000))?,
            "canonicalizationVersion": "context-view-v2",
            "request": request,
            "items": registrations.iter().map(|registration| json!({
                "contextId": registration.id,
                "version": registration.version,
                "required": true,
                "utility": 1
            })).collect::<Vec<_>>()
        });
        let view = match adapter.client.product_view(&plan, &id).await {
            Ok(view) => view,
            Err(error) => {
                let _ = adapter.client.product_get("view", &id).await;
                return Err(error);
            }
        };
        if view.schema_version != 1
            || view.allocation_id != capability.allocation_id
            || view.runtime != capability.runtime
            || view.release != capability.release
            || view.lease_epoch != capability.lease_epoch
            || view.canonicalization_version != "context-view-v2"
            || view.request_digest.as_deref() != Some(&request_digest)
            || view.data_epoch != measured["dataEpoch"].as_u64()
            || view.state != "ready"
            || view.token_count > capability.input_limit()?
            || view.input_budget_tokens > capability.input_limit()?
            || !view.omitted.is_empty()
            || view.ordered_items.len() != registrations.len()
            || view.ordered_items.iter().any(|item| {
                !item.required
                    || !registrations.iter().any(|registration| {
                        registration.id == item.context_id
                            && registration.version == item.version
                            && registration.source_digest == item.source_digest
                    })
            })
        {
            return Err("personal-view-binding".into());
        }
        let receipt_value = adapter.client.product_get("view", &id).await?;
        if receipt_value["subjectDigest"] != capability.subject_digest
            || receipt_value["bootEpoch"] != capability.boot_epoch
            || receipt_value["viewId"] != view.id
            || receipt_value["requestDigest"] != request_digest
            || receipt_value["state"] != "ready"
        {
            return Err("personal-view-receipt".into());
        }
        receipt(adapter, &id, &receipt_value)?;
        manifest.expires_at = manifest
            .expires_at
            .min(saaa_larm_session::personal_state::instant(
                &view.expires_at,
            )?);
        manifest.view_id = Some(view.id);
        manifest.view_digest = Some(view.view_digest);
    }
    Ok(Prepared { request_digest })
}
