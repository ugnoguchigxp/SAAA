use crate::database_error;
use rusqlite::{params, Connection};
use saaa_personal_state_core::{
    AccessScope, Classification, Purpose, SourceKey, SourceRef, SourceRole,
};
use sha2::{Digest, Sha256};

#[derive(Debug)]
pub struct Chunk {
    pub source: SourceRef,
    pub text: String,
    pub total_bytes: u64,
}

/// Stable save-sequence pagination. Each page is bounded, with no 400-message cap.
pub fn page(c: &Connection, after: u64, limit: usize) -> Result<Vec<u64>, String> {
    let mut s=c.prepare("SELECT sequence FROM personal_sources WHERE sequence>?1 AND available=1 AND bytes>0 ORDER BY sequence LIMIT ?2").map_err(database_error)?;
    let result = s
        .query_map(params![after, limit.clamp(1, 128)], |r| r.get(0))
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error);
    result
}

/// Read exact UTF-8 bytes from the canonical source; a partial final codepoint is
/// deferred to the next chunk. Unknown or edited versions fail before returning text.
pub fn load(c: &Connection, sequence: u64, offset: u64, max_bytes: usize) -> Result<Chunk, String> {
    if !(4..=262144).contains(&max_bytes) {
        return Err("personal-source-budget".into());
    }
    let (id,version,role,total,at,raw):(String,u64,String,u64,i64,Vec<u8>)=c.query_row(
        "SELECT s.message_id,s.version,s.role,s.bytes,s.recorded_at,substr(CAST(m.content AS BLOB),?2+1,?3) FROM personal_sources s JOIN conversation_messages m ON m.id=s.message_id WHERE s.sequence=?1 AND s.available=1 AND NOT EXISTS(SELECT 1 FROM personal_tombstones t WHERE t.source_id=s.message_id)",params![sequence,offset,max_bytes],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).map_err(|_|"personal-source-unavailable")?;
    if offset >= total {
        return Err("personal-source-range".into());
    }
    let end = match std::str::from_utf8(&raw) {
        Ok(_) => raw.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => return Err("personal-source-utf8".into()),
    };
    if end == 0 {
        return Err("personal-source-range".into());
    }
    let text = std::str::from_utf8(&raw[..end])
        .map_err(|_| "personal-source-utf8")?
        .to_string();
    let (principal, policy): (String, u64) = c
        .query_row(
            "SELECT principal,policy_revision FROM personal_scope WHERE id='primary'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(database_error)?;
    let source = SourceRef {
        key: SourceKey {
            id,
            version,
            start: offset,
            end: offset + end as u64,
        },
        digest: format!("{:x}", Sha256::digest(text.as_bytes())),
        sequence,
        recorded_at: at,
        role: match role.as_str() {
            "user" | "transcript" => SourceRole::User,
            _ => SourceRole::Assistant,
        },
        access: AccessScope {
            principal,
            scope: "primary".into(),
            task_request: None,
            purposes: [
                Purpose::Reasoning,
                Purpose::StateExtract,
                Purpose::Diagnostics,
            ]
            .into_iter()
            .collect(),
            classification: Classification::Confidential,
            policy_revision: policy,
        },
        available: true,
        valid_until: None,
        finalized: offset == 0 && end as u64 == total,
    };
    Ok(Chunk {
        source,
        text,
        total_bytes: total,
    })
}

pub fn revalidate(c: &Connection, source: &SourceRef) -> Result<(), String> {
    let chunk = load(
        c,
        source.sequence,
        source.key.start,
        (source.key.end - source.key.start).max(4) as usize,
    )?;
    if chunk.source.key != source.key
        || chunk.source.digest != source.digest
        || chunk.source.access != source.access
    {
        return Err("personal-source-changed".into());
    }
    Ok(())
}

/// Used only in a successful full-message finalization transaction. It does not
/// change source identity/content; it certifies that surrounding text was read.
pub fn finalize(c: &Connection, full: &SourceRef) -> Result<(), String> {
    if !full.finalized || full.key.start != 0 {
        return Err("personal-finalization-range".into());
    }
    revalidate(c, full)?;
    c.execute("UPDATE personal_source_refs SET metadata=json_set(metadata,'$.finalized',json('true')) WHERE json_extract(metadata,'$.key.id')=?1 AND json_extract(metadata,'$.key.version')=?2 AND json_extract(metadata,'$.key.end')<=?3",params![full.key.id,full.key.version,full.key.end]).map_err(database_error)?;
    Ok(())
}
