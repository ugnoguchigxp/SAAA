use crate::database_error;
use rusqlite::{params, Connection};
use saaa_personal_state_core::{AccessRequest, Classification, Purpose, Status};
use serde_json::{json, Value};

/// Required current state and pending originals are never silently truncated.
/// This is JSON data, never a new instruction source.
pub fn compose(c: &Connection, task: Option<&str>, max_bytes: usize) -> Result<Value, String> {
    let ledger = super::store::load(c)?;
    let request = AccessRequest {
        principal: &ledger.principal,
        scope: "primary",
        task_request: task,
        purpose: Purpose::Reasoning,
        max_classification: Classification::Confidential,
        policy_revision: ledger.policy_revision,
        authorized: true,
    };
    let projection = ledger
        .project(&request, super::now())
        .map_err(|_| "personal-projection-unauthorized")?;
    let mut items = Vec::new();
    for (id, status) in projection.items {
        if !matches!(
            status,
            Status::Active | Status::Candidate | Status::Disputed
        ) {
            continue;
        }
        let a = &ledger.assertions[&id];
        let newest_support = a
            .input_dependencies
            .iter()
            .filter_map(|key| ledger.sources.get(key).map(|s| s.sequence))
            .max()
            .unwrap_or(0);
        let pending_review: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM personal_sources p JOIN personal_jobs j ON j.source_sequence=p.sequence WHERE p.available=1 AND p.bytes>0 AND p.sequence>?1 AND j.status!='completed')", [newest_support], |r| r.get(0)).map_err(database_error)?;
        let status = if status == Status::Active && pending_review {
            Status::Candidate
        } else {
            status
        };
        let value: String = c
            .query_row(
                "SELECT value_json FROM personal_payloads WHERE id=?1",
                [&a.payload_ref],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        items.push(json!({"id":id,"kind":a.kind,"key":a.semantic_key,"value":super::decode::<Value>(value)?,"status":status,"evidence":a.evidence}));
    }
    let mut pending = Vec::new();
    let mut s=c.prepare("SELECT p.sequence FROM personal_sources p JOIN personal_jobs j ON j.source_sequence=p.sequence WHERE p.available=1 AND j.status!='completed' ORDER BY p.sequence").map_err(database_error)?;
    let sequences = s
        .query_map([], |r| r.get::<_, u64>(0))
        .map_err(database_error)?;
    let mut used = super::encode(&items)?.len();
    for seq in sequences {
        let seq = seq.map_err(database_error)?;
        let bytes: u64 = c
            .query_row(
                "SELECT bytes FROM personal_sources WHERE sequence=?1",
                params![seq],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        if bytes == 0 {
            continue;
        }
        if bytes as usize > max_bytes.saturating_sub(used) || bytes > 262144 {
            return Err("personal-continuity-incomplete".into());
        }
        let chunk = super::sources::load(c, seq, 0, (bytes as usize).max(4))?;
        if !chunk.source.finalized {
            return Err("personal-continuity-incomplete".into());
        }
        let value = json!({"source":chunk.source,"text":chunk.text});
        used += super::encode(&value)?.len();
        if used > max_bytes {
            return Err("personal-continuity-incomplete".into());
        }
        pending.push(value);
    }
    let value = json!({"instructionAuthority":"none","revision":ledger.revision,"inputEpoch":ledger.input_epoch,"items":items,"pending":pending});
    if super::encode(&value)?.len() > max_bytes {
        return Err("personal-continuity-incomplete".into());
    }
    Ok(value)
}
