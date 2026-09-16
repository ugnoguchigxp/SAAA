//! The local owner is never sent to LARM. Its authenticated subject pin lives in Keychain.
use super::{contract::Certification, managed::Adapter};
use crate::{database_error, persistence::sqlite::SqliteWriter};
use saaa_larm_session::{contexts::Client, personal_state::Capability, Session};
use std::sync::Arc;
use tokio::sync::{watch, Mutex};
static SESSION: Mutex<Option<Arc<Session>>> = Mutex::const_new(None);

pub async fn configure(writer: Arc<SqliteWriter>) -> Result<Adapter, String> {
    let session =
        if let Ok(ready) = crate::larm_voice::current(crate::PRIMARY_CONVERSATION_ID).await {
            ready.session.clone()
        } else {
            let mut cached = SESSION.lock().await;
            if cached.is_none() {
                let base = std::env::var("SAAA_LARM_CONTROL_URL")
                    .unwrap_or_else(|_| "http://gnosis.local:9810".into());
                let (_alive, rx) = watch::channel(false);
                // Keep the sender alive for the entire creation handshake.
                *cached = Some(
                    Session::connect(&base, rx)
                        .await
                        .map_err(|_| "personal-connection-unavailable")?,
                );
            }
            cached
                .as_ref()
                .ok_or("personal-connection-unavailable")?
                .clone()
        };
    let lease = match session.acquire("llm").await {
        Ok(lease) => lease,
        Err(_) => {
            *SESSION.lock().await = None;
            return Err("personal-connection-unavailable".into());
        }
    };
    let subject = lease.context_subject().map_err(str::to_string)?.to_string();
    let endpoint = lease.provider().base_url.origin().ascii_serialization();
    let client = Client::new(&endpoint, lease.provider().token().to_string())?;
    let runtime = std::env::var("SAAA_PERSONAL_STATE_RUNTIME")
        .unwrap_or_else(|_| "qwen-worker-quality".into());
    let (cap, can_generate) = match client
        .personal_capability(lease.allocation_id(), &runtime)
        .await
    {
        Ok(cap) => {
            cap.validate(&subject, lease.allocation_id(), &runtime, super::now())?;
            (cap, true)
        }
        Err(_) => {
            // OFF still permits authenticated forget and receipt reconciliation. Cached
            // capability metadata grants no authority to create sources or generations.
            let cap: Capability = writer.read_serialized(|c| {
                let raw: String = c
                    .query_row(
                        "SELECT capability FROM personal_product_binding WHERE id=1 AND subject=?1",
                        [&subject],
                        |r| r.get(0),
                    )
                    .map_err(|_| "personal-capability-unavailable")?;
                super::decode(raw)
            })?;
            (cap, false)
        }
    };
    let owner = writer.read_serialized(|c| {
        c.query_row("SELECT principal FROM personal_scope", [], |r| {
            r.get::<_, String>(0)
        })
        .map_err(database_error)
    })?;
    super::subject_pin::pin_subject(&owner, &subject)?;
    let mut cert = certification(&cap, &owner, &endpoint, &lease.provider().model)?;
    if !can_generate {
        cert.source_delivery_verified = false;
    }
    writer.write(|c| {
        let tx=c.transaction().map_err(database_error)?;
        let prior:Option<String>=tx.query_row("SELECT subject FROM personal_product_binding WHERE id=1",[],|r|r.get(0)).optional().map_err(database_error)?;
        if prior.as_ref().is_some_and(|s|s!=&subject){return Err("personal-subject-changed".into());}
        tx.execute("INSERT INTO personal_product_binding VALUES(1,?1,?2,?3) ON CONFLICT(id) DO UPDATE SET capability=excluded.capability,updated_at=excluded.updated_at",rusqlite::params![subject,super::encode(&cap)?,super::now()]).map_err(database_error)?;
        tx.execute("INSERT INTO personal_contract VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET value_json=excluded.value_json",[super::encode(&cert)?]).map_err(database_error)?;
        tx.commit().map_err(database_error)
    })?;
    Ok(Adapter {
        writer,
        certification: cert,
        client,
        delivery: Arc::new(super::managed::UnavailableDelivery),
        product: Some(super::product::Product {
            can_generate,
            capability: cap,
            _lease: Some(lease),
        }),
    })
}
use rusqlite::OptionalExtension;
fn certification(
    c: &Capability,
    owner: &str,
    endpoint: &str,
    model: &str,
) -> Result<Certification, String> {
    Ok(Certification {
        model: model.into(),
        principal: owner.into(),
        release: c.release.clone(),
        runtime: c.runtime.clone(),
        allocation: c.allocation_id.clone(),
        endpoint: endpoint.into(),
        tokenizer_digest: c.tokenizer_digest.clone(),
        capability: "llm.coding".into(),
        native_tokens: c.context_limit_tokens,
        output_reserve: c.output_limit()?,
        safety_margin: c.safety_margin_tokens,
        max_input_tokens: c.input_limit()?,
        max_bytes: usize::try_from(c.max_materialized_bytes).map_err(|_| "personal-byte-budget")?,
        expires_at: c.expires_at()?,
        lease_epoch: c.lease_epoch,
        source_delivery_verified: true,
        cleanup_verified: true,
        base_snapshot_safe: true,
        semantic_verified: true,
        cancellation_verified: true,
    })
}
pub async fn shutdown() {
    if let Some(session) = SESSION.lock().await.take() {
        let _ = session.close().await;
    }
}
