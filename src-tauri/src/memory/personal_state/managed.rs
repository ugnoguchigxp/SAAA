//! Coordinates the verified LARM HTTP contract and a separately supplied source
//! delivery/canonical-measurement contract. No SSH or unpublished HTTP API is inferred.
use super::{contract::Certification, generation, worker};
use crate::{database_error, persistence::sqlite::SqliteWriter, RunCancellation};
use async_trait::async_trait;
use rusqlite::params;
use saaa_larm_session::contexts::{Client, PlanItem, Registration, ViewRequest};
use saaa_personal_state_core::{Provenance, SourceRef};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct Provision {
    pub handle: String,
    pub digest: String,
    pub bytes: u64,
    pub tokens: u64,
    pub tokenizer: String,
}
#[async_trait]
pub trait Delivery: Send + Sync {
    async fn measure(&self, messages: &Value, cancel: Arc<RunCancellation>) -> Result<u64, String>;
    async fn provision(
        &self,
        source: &SourceRef,
        text: &str,
        cancel: Arc<RunCancellation>,
    ) -> Result<Provision, String>;
    async fn erase(&self, handle: &str) -> Result<(bool, bool), String>;
}
pub struct UnavailableDelivery;
#[async_trait]
impl Delivery for UnavailableDelivery {
    async fn measure(&self, _: &Value, _: Arc<RunCancellation>) -> Result<u64, String> {
        Err("personal-canonical-measurement-unconfigured".into())
    }
    async fn provision(
        &self,
        _: &SourceRef,
        _: &str,
        _: Arc<RunCancellation>,
    ) -> Result<Provision, String> {
        Err("personal-source-delivery-unconfigured".into())
    }
    async fn erase(&self, _: &str) -> Result<(bool, bool), String> {
        Err("personal-source-cleanup-unconfigured".into())
    }
}
struct Prepared {
    incarnation: String,
    source: SourceRef,
    text: String,
    messages: Value,
    base_tokens: u64,
    run_id: Option<String>,
    exposed: Vec<SourceRef>,
}
pub struct Adapter {
    pub product: Option<super::product::Product>,
    pub writer: Arc<SqliteWriter>,
    pub certification: Certification,
    pub client: Client,
    pub delivery: Arc<dyn Delivery>,
}
impl Adapter {
    pub async fn configured(writer: Arc<SqliteWriter>) -> Result<Self, String> {
        super::product_binding::configure(writer).await
    }
    async fn execute(&self, input: Value, cancel: Arc<RunCancellation>) -> Result<String, String> {
        let source: SourceRef = serde_json::from_value(input["source"]["ref"].clone())
            .map_err(|_| "personal-extraction-source")?;
        let text = input["source"]["text"]
            .as_str()
            .ok_or("personal-extraction-source")?;
        self.certification
            .check(&source.access.principal, super::now())?;
        let messages = json!([{"role":"system","content":input["instruction"]},{"role":"user","content":super::encode(&json!({"current":input["current"],"source_ref":source.key,"request_scope":input["request_scope"],"instructionAuthority":"none"}))?}]);
        let base_tokens = self.delivery.measure(&messages, cancel.clone()).await?;
        self.certification
            .budget()
            .validate_materialized(base_tokens, super::encode(&messages)?.len() as u64, 2000)
            .map_err(|_| "personal-input-budget")?;
        let incarnation = crate::new_id("registration");
        self.writer.write(|c|{super::sources::revalidate(c,&source)?;c.execute("INSERT INTO personal_registrations VALUES(?1,?2,?3,?1,'{}','active','provisioning',1,?4)",params![incarnation,source.key.id,source.key.version,self.certification.expires_at]).map_err(database_error)?;c.execute("INSERT OR IGNORE INTO personal_cleanup(incarnation,source_id) VALUES(?1,?2)",params![incarnation,source.key.id]).map_err(database_error)?;Ok(())})?;
        let mut exposed = Vec::new();
        if let Some(pending) = input["current"]["pending"].as_array() {
            for entry in pending {
                exposed.push(
                    serde_json::from_value(entry["source"].clone())
                        .map_err(|_| "personal-base-source")?,
                );
            }
        }
        let prepared = Prepared {
            incarnation: incarnation.clone(),
            source: source.clone(),
            text: text.into(),
            messages,
            base_tokens,
            run_id: input["run_id"].as_str().map(str::to_string),
            exposed,
        };
        let result = self.register_and_generate(prepared, cancel.clone()).await;
        // Every transient copy, including ambiguous/late registration, has an outbox.
        self.writer.write(|c| {
            c.execute(
                "INSERT OR IGNORE INTO personal_cleanup(incarnation,source_id) VALUES(?1,?2)",
                params![incarnation, source.key.id],
            )
            .map_err(database_error)?;
            c.execute(
                "UPDATE personal_registrations SET desired='deleted',pins=0 WHERE incarnation=?1",
                [&incarnation],
            )
            .map_err(database_error)?;
            Ok(())
        })?;
        result
    }
    async fn register_and_generate(
        &self,
        prepared: Prepared,
        cancel: Arc<RunCancellation>,
    ) -> Result<String, String> {
        let inc = prepared.incarnation.as_str();
        let source = &prepared.source;
        let text = prepared.text.as_str();
        let messages = prepared.messages;
        let base_tokens = prepared.base_tokens;
        let provision = self
            .delivery
            .provision(source, text, cancel.clone())
            .await?;
        // Preserve cleanup capability even if forgetting races with provisioning,
        // or the attestation is rejected. Do not persist returned source content.
        self.writer.write(|c| {
            c.execute(
                "UPDATE personal_registrations SET metadata=?2 WHERE incarnation=?1",
                params![
                    inc,
                    super::encode(&json!({"sourceHandle":provision.handle}))?
                ],
            )
            .map_err(database_error)?;
            Ok(())
        })?;
        if provision.digest != source.digest
            || provision.bytes != text.len() as u64
            || provision.tokenizer != self.certification.tokenizer_digest
            || provision.tokens > 20_000_000
        {
            return Err("personal-provision-attestation".into());
        }
        let registration = Registration {
            id: inc.into(),
            version: source.key.version.to_string(),
            source_handle: provision.handle,
            source_digest: provision.digest,
            classification: "confidential".into(),
            byte_count: provision.bytes,
            token_count: provision.tokens,
            tokenizer_digest: provision.tokenizer,
            expires_at: Some(iso(self.certification.expires_at)?),
        };
        self.writer.write(|c|{super::sources::revalidate(c,source)?;c.execute("UPDATE personal_registrations SET metadata=?2,observed='registering' WHERE incarnation=?1",params![inc,super::encode(&registration)?]).map_err(database_error)?;Ok(())})?;
        let descriptor = self.client.register(&registration, inc).await?;
        for (key, value) in serde_json::to_value(&registration)
            .map_err(|_| "personal-registration-encoding")?
            .as_object()
            .ok_or("personal-registration-encoding")?
        {
            if descriptor.get(key) != Some(value) {
                return Err("personal-registration-unconfirmed".into());
            }
        }
        if descriptor["id"] != inc
            || descriptor["sourceDigest"] != source.digest
            || descriptor["state"] != "active"
        {
            return Err("personal-registration-unconfirmed".into());
        }
        self.writer.write(|c|{super::sources::revalidate(c,source)?;c.execute("UPDATE personal_registrations SET observed='active' WHERE incarnation=?1 AND desired='active'",[inc]).map_err(database_error)?;Ok(())})?;
        let request = ViewRequest {
            allocation_id: self.certification.allocation.clone(),
            runtime: self.certification.runtime.clone(),
            base_input_tokens: base_tokens,
            max_input_tokens: self.certification.max_input_tokens,
            deadline: iso(super::now() + 30000)?,
            canonicalization_version: "context-view-v1".into(),
            items: vec![PlanItem {
                context_id: inc.into(),
                version: source.key.version.to_string(),
                required: true,
                utility: 1.0,
            }],
        };
        let view = self
            .client
            .create_view(&request, &crate::new_id("view_request"))
            .await?;
        view.validate(
            &request,
            &self.certification.release,
            self.certification.lease_epoch,
            super::now(),
        )?;
        if view
            .ordered_items
            .iter()
            .any(|s| s.source_digest != source.digest)
        {
            return Err("personal-view-source-digest".into());
        }
        let manifest = self.writer.write(|c| {
            let tx = c.transaction().map_err(database_error)?;
            let ledger = super::store::load(&tx)?;
            let mut sources = prepared.exposed;
            if !sources.iter().any(|s| s.key == source.key) {
                sources.push(source.clone());
            }
            for a in ledger.assertions.values() {
                for k in &a.input_dependencies {
                    if let Some(s) = ledger.sources.get(k) {
                        if !sources.iter().any(|prior| prior.key == s.key) {
                            sources.push(s.clone());
                        }
                    }
                }
            }
            let m = generation::Manifest {
                request_digest: generation::request_digest(&json!({"model":self.certification.model,"messages":messages,"max_tokens":2000,"stream":false}))?,
                generation_id: crate::new_id("generation"),
                attempt_id: crate::new_id("attempt"),
                run_id: prepared
                    .run_id
                    .clone()
                    .unwrap_or_else(|| format!("extract-{inc}")),
                request_revision: 1,
                input_epoch: ledger.input_epoch,
                policy_revision: ledger.policy_revision,
                projection_revision: ledger.revision,
                purpose: if prepared.run_id.is_some() {
                    "reasoning"
                } else {
                    "personal_state_extract"
                }
                .into(),
                sources,
                allocation: view.allocation_id.clone(),
                runtime: view.runtime.clone(),
                release: view.release.clone(),
                view_id: Some(view.id.clone()),
                view_digest: Some(view.view_digest.clone()),
                lease_epoch: view.lease_epoch,
                expires_at: chrono::DateTime::parse_from_rfc3339(&view.expires_at)
                    .map_err(|_| "personal-view-expiry")?
                    .timestamp_millis(),
            };
            generation::prepare(&tx, &m)?;
            generation::dispatch(&tx, &m.generation_id)?;
            tx.commit().map_err(database_error)?;
            Ok(m)
        })?;
        let chat_request = json!({"model":self.certification.model,"messages":messages,"max_tokens":2000,"stream":false});
        let response = tokio::select! {biased;_=cancel.cancelled()=>{self.writer.write(|c|{c.execute("UPDATE personal_generations SET output_allowed=0,cancellation='sent-unconfirmed',status='cancelled' WHERE id=?1",[&manifest.generation_id]).map_err(database_error)?;Ok(())})?;return Err("personal-generation-cancelled".into());},response=self.client.chat(&view,&self.certification.capability,&chat_request)=>response};
        let accepted = match response {
            Ok(response) => {
                let operation = self.client.operation(&view.operation_id).await?;
                if operation["state"] != "succeeded"
                    || response["choices"][0]["finish_reason"] != "stop"
                {
                    Err("personal-materialization-unconfirmed".into())
                } else {
                    self.writer.write(|c|{generation::allow(c,&manifest.generation_id)?;c.execute("UPDATE personal_generations SET materialization='succeeded' WHERE id=?1",[&manifest.generation_id]).map_err(database_error)?;Ok(())})?;
                    response["choices"][0]["message"]["content"]
                        .as_str()
                        .filter(|s| s.len() <= 16384)
                        .map(str::to_string)
                        .ok_or_else(|| "personal-generation-output".into())
                }
            }
            Err(e) => Err(e),
        };
        self.writer
            .write(|c| generation::finish(c, &manifest.generation_id, accepted.is_ok()))?;
        accepted
    }
    pub async fn cleanup(&self) -> Result<usize, String> {
        if self.product.is_some() {
            return super::product_cleanup::cleanup(self).await;
        }
        let pending=self.writer.read_serialized(|c|{let mut s=c.prepare("SELECT r.incarnation,r.context_id,r.metadata FROM personal_registrations r JOIN personal_cleanup q USING(incarnation) WHERE q.stage!='complete' AND r.pins=0 ORDER BY r.incarnation LIMIT 16").map_err(database_error)?;let rows=s.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).map_err(database_error)?.collect::<Result<Vec<_>,_>>().map_err(database_error)?;Ok(rows)})?;
        let mut complete = 0;
        for (inc, id, metadata) in pending {
            self.client
                .delete_registration(&id, &format!("delete-{inc}"))
                .await?;
            let absent = self.client.registration_absent(&id).await?;
            let metadata: Value =
                serde_json::from_str(&metadata).map_err(|_| "personal-cleanup-metadata")?;
            let (source_absent, snapshot_safe) =
                if let Some(handle) = metadata["sourceHandle"].as_str() {
                    self.delivery.erase(handle).await.unwrap_or((false, false))
                } else {
                    (false, false)
                };
            self.writer.write(|c| {
                generation::cleanup_confirm(c, &inc, absent, source_absent, snapshot_safe)
            })?;
            if absent && source_absent && snapshot_safe {
                complete += 1;
            }
        }
        Ok(complete)
    }
}
#[async_trait]
impl worker::Extractor for Adapter {
    fn owns_cancellation(&self) -> bool {
        self.product.is_some()
    }
    async fn extract(&self, input: Value, cancel: Arc<RunCancellation>) -> Result<String, String> {
        if self.product.is_some() {
            super::product_extract::extract(self, input, cancel).await
        } else {
            self.execute(input, cancel).await
        }
    }
    async fn extract_world(
        &self,
        input: Value,
        cancel: Arc<RunCancellation>,
    ) -> Result<Option<String>, String> {
        if self.product.is_none() {
            return Ok(None);
        }
        super::product_extract::extract(self, input, cancel)
            .await
            .map(Some)
    }
    fn provenance(&self) -> Provenance {
        Provenance {
            model: self.certification.model.clone(),
            release: self.certification.release.clone(),
            extractor_version: "p1-v1".into(),
            prompt_digest: format!(
                "{:x}",
                Sha256::digest(worker::EXTRACTION_INSTRUCTION.as_bytes())
            ),
            schema_version: "p1-v1".into(),
            config_digest: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&self.certification).unwrap_or_default())
            ),
            runtime_event: None,
        }
    }
}
pub(super) fn iso(at: i64) -> Result<String, String> {
    chrono::DateTime::from_timestamp_millis(at)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or_else(|| "personal-time-invalid".into())
}
