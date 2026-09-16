use crate::database_error;
use rusqlite::{params, Connection};
use saaa_personal_state_core::SourceRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub generation_id: String,
    pub attempt_id: String,
    pub run_id: String,
    pub request_revision: u64,
    pub input_epoch: u64,
    pub policy_revision: u64,
    pub projection_revision: u64,
    pub purpose: String,
    #[serde(default)]
    pub request_digest: String,
    pub sources: Vec<SourceRef>,
    pub allocation: String,
    pub runtime: String,
    pub release: String,
    pub view_id: Option<String>,
    pub view_digest: Option<String>,
    pub lease_epoch: u64,
    pub expires_at: i64,
}
pub fn prepare(c: &Connection, m: &Manifest) -> Result<(), String> {
    let current: (u64, u64, u64) = c
        .query_row(
            "SELECT input_epoch,policy_revision,revision FROM personal_scope",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(database_error)?;
    if current != (m.input_epoch, m.policy_revision, m.projection_revision) {
        return Err("personal-generation-stale".into());
    }
    for s in &m.sources {
        super::sources::revalidate(c, s)?;
    }
    c.execute("INSERT INTO personal_generations(id,run_id,attempt_id,request_revision,input_epoch,policy_revision,projection_revision,purpose,manifest_json,view_id,status) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'prepared')",params![m.generation_id,m.run_id,m.attempt_id,m.request_revision,m.input_epoch,m.policy_revision,m.projection_revision,m.purpose,super::encode(m)?,m.view_id]).map_err(database_error)?;
    for s in &m.sources {
        c.execute(
            "INSERT OR IGNORE INTO personal_generation_inputs VALUES(?1,?2,?3)",
            params![m.generation_id, s.key.id, s.key.version],
        )
        .map_err(database_error)?;
    }
    Ok(())
}
pub fn allow(c: &Connection, id: &str) -> Result<(), String> {
    let valid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_generations g JOIN personal_scope s WHERE g.id=?1 AND g.output_allowed=1 AND json_extract(g.manifest_json,'$.expires_at')>?2 AND g.policy_revision=s.policy_revision AND g.input_epoch=s.input_epoch AND s.recovery_ready=1 AND NOT EXISTS(SELECT 1 FROM personal_generation_inputs i LEFT JOIN personal_sources p ON p.message_id=i.source_id AND p.version=i.version AND p.available=1 WHERE i.generation_id=g.id AND (p.sequence IS NULL OR NOT EXISTS(SELECT 1 FROM conversation_messages m WHERE m.id=i.source_id) OR EXISTS(SELECT 1 FROM personal_tombstones t WHERE t.source_id=i.source_id))))",params![id,super::now()],|r|r.get(0)).map_err(database_error)?;
    if !valid {
        return Err("personal-output-invalidated".into());
    }
    Ok(())
}
pub fn dispatch(c: &Connection, id: &str) -> Result<(), String> {
    allow(c, id)?;
    let expires:i64=c.query_row("SELECT json_extract(manifest_json,'$.expires_at') FROM personal_generations WHERE id=?1",[id],|r|r.get(0)).map_err(database_error)?;
    if expires <= super::now() {
        return Err("personal-view-expired".into());
    }
    let n = c
        .execute(
            "UPDATE personal_generations SET status='running' WHERE id=?1 AND status='prepared'",
            [id],
        )
        .map_err(database_error)?;
    if n != 1 {
        return Err("personal-view-consumed".into());
    }
    Ok(())
}
pub fn finish(c: &Connection, id: &str, succeeded: bool) -> Result<(), String> {
    // Metadata survives rejection; no result body belongs in this table.
    c.execute(
        "UPDATE personal_generations SET status=?2 WHERE id=?1",
        params![id, if succeeded { "succeeded" } else { "failed" }],
    )
    .map_err(database_error)?;
    Ok(())
}
pub fn allow_run(c: &Connection, run: &str) -> Result<(), String> {
    let invalid:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_generations WHERE run_id=?1 AND output_allowed=0)",[run],|r|r.get(0)).map_err(database_error)?;
    if invalid {
        return Err("personal-output-invalidated".into());
    }
    Ok(())
}

pub fn cleanup_confirm(
    c: &Connection,
    incarnation: &str,
    registration_absent: bool,
    source_absent: bool,
    snapshot_safe: bool,
) -> Result<(), String> {
    c.execute("UPDATE personal_cleanup SET registration_absent=?2,source_absent=?3,snapshot_safe=?4,stage=CASE WHEN ?2 AND ?3 AND ?4 THEN 'complete' ELSE 'pending' END,last_code=CASE WHEN ?2 AND ?3 AND ?4 THEN 'confirmed' ELSE 'remote-cleanup-pending' END WHERE incarnation=?1",params![incarnation,registration_absent,source_absent,snapshot_safe]).map_err(database_error)?;
    Ok(())
}

/// Rechecked inside each existing tool writer transaction, not only before calling it.
pub fn allow_dispatch(c: &Connection, run: &str) -> Result<(), String> {
    use rusqlite::OptionalExtension;
    allow_run(c, run)?;
    let latest: Option<String> = c
        .query_row(
            "SELECT id FROM personal_generations WHERE run_id=?1 ORDER BY rowid DESC LIMIT 1",
            [run],
            |r| r.get(0),
        )
        .optional()
        .map_err(database_error)?;
    if let Some(id) = latest {
        allow(c, &id)?;
    }
    Ok(())
}

pub fn request_digest(request: &serde_json::Value) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(super::encode(request)?.as_bytes())
    ))
}
