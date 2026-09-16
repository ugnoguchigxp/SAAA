//! Transient registration ownership: every await/failure/drop retains cleanup work.
use super::{managed::Adapter, sources};
use crate::{database_error, RunCancellation};
use rusqlite::params;
use saaa_larm_session::contexts::Registration;
use saaa_personal_state_core::SourceRef;
use serde_json::json;
use std::sync::Arc;

pub(super) struct Pins<'a> {
    adapter: &'a Adapter,
    ids: Vec<String>,
}
impl<'a> Pins<'a> {
    pub fn new(adapter: &'a Adapter) -> Self {
        Self {
            adapter,
            ids: Vec::new(),
        }
    }
    pub async fn register(
        &mut self,
        source: &SourceRef,
        text: &str,
        cancel: Arc<RunCancellation>,
    ) -> Result<Registration, String> {
        self.adapter
            .certification
            .check(&source.access.principal, super::now())?;
        let id = crate::new_id("registration");
        self.adapter.writer.write(|c| {
            let tx=c.transaction().map_err(database_error)?;
            sources::revalidate(&tx,source)?;
            tx.execute("INSERT INTO personal_registrations VALUES(?1,?2,?3,?1,'{}','active','provisioning',1,?4)",params![id,source.key.id,source.key.version,self.adapter.certification.expires_at]).map_err(database_error)?;
            tx.execute("INSERT INTO personal_cleanup(incarnation,source_id) VALUES(?1,?2)",params![id,source.key.id]).map_err(database_error)?;
            tx.commit().map_err(database_error)
        })?;
        self.ids.push(id.clone());
        let p = self
            .adapter
            .delivery
            .provision(source, text, cancel)
            .await?;
        self.adapter.writer.write(|c| {
            c.execute(
                "UPDATE personal_registrations SET metadata=?2 WHERE incarnation=?1",
                params![id, super::encode(&json!({"sourceHandle":p.handle}))?],
            )
            .map_err(database_error)?;
            Ok(())
        })?;
        if p.digest != source.digest
            || p.bytes != text.len() as u64
            || p.tokenizer != self.adapter.certification.tokenizer_digest
            || p.tokens > 20_000_000
        {
            return Err("personal-provision-attestation".into());
        }
        let r = Registration {
            id: id.clone(),
            version: source.key.version.to_string(),
            source_handle: p.handle,
            source_digest: p.digest,
            classification: "confidential".into(),
            byte_count: p.bytes,
            token_count: p.tokens,
            tokenizer_digest: p.tokenizer,
            expires_at: Some(super::managed::iso(self.adapter.certification.expires_at)?),
        };
        self.adapter.writer.write(|c| {
            sources::revalidate(c,source)?;
            c.execute("UPDATE personal_registrations SET metadata=?2,observed='registering' WHERE incarnation=?1 AND desired='active'",params![id,super::encode(&r)?]).map_err(database_error)?;
            Ok(())
        })?;
        let response = self.adapter.client.register(&r, &id).await?;
        let expected = serde_json::to_value(&r).map_err(|_| "personal-registration-schema")?;
        if response["state"] != "active"
            || expected
                .as_object()
                .ok_or("personal-registration-schema")?
                .iter()
                .any(|(k, v)| response.get(k) != Some(v))
        {
            return Err("personal-registration-unconfirmed".into());
        }
        self.adapter.writer.write(|c| {
            sources::revalidate(c,source)?;
            let count=c.execute("UPDATE personal_registrations SET observed='active' WHERE incarnation=?1 AND desired='active'",[&id]).map_err(database_error)?;
            if count!=1{return Err("personal-registration-invalidated".into());} Ok(())
        })?;
        Ok(r)
    }
}
impl Drop for Pins<'_> {
    fn drop(&mut self) {
        let _=self.adapter.writer.write(|c| {
            for id in &self.ids {c.execute("UPDATE personal_registrations SET pins=0,desired='deleted' WHERE incarnation=?1",[id]).map_err(database_error)?;}
            Ok(())
        });
    }
}
