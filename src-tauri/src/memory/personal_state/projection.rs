use crate::database_error;
use rusqlite::{params, Connection};
use saaa_personal_state_core::Status;
#[cfg(test)]
use saaa_personal_state_core::{AccessRequest, Classification, Purpose};
use serde_json::{json, Value};

pub(crate) fn context_candidates(
    c: &Connection,
    scope: &crate::runtime::context::scope::ScopeSnapshot,
    current_message_id: &str,
    max_bytes: usize,
) -> Result<Vec<crate::runtime::context::source::Candidate>, String> {
    use crate::runtime::context::source::{Candidate, Requirement};
    let ledger = super::store::load(c)?;
    let scope_keys = scope
        .scopes
        .iter()
        .map(|value| value.key.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut candidates = Vec::new();
    for assertion in ledger.assertions.values() {
        if assertion.kind.is_world() {
            continue;
        }
        let assertion_scopes = assertion
            .access
            .task_request
            .as_deref()
            .map(canonical_scope_key)
            .into_iter()
            .collect::<Vec<_>>();
        if assertion.access.task_request.is_some()
            && !assertion_scopes
                .iter()
                .any(|key| scope_keys.contains(key.as_str()))
        {
            continue;
        }
        let status = ledger.status(&assertion.id, super::now());
        if !matches!(
            status,
            Status::Active | Status::Candidate | Status::Disputed
        ) {
            continue;
        }
        let value: String = c
            .query_row(
                "SELECT value_json FROM personal_payloads WHERE id=?1",
                [&assertion.payload_ref],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let content = serde_json::to_string(&json!({
            "id": assertion.id,
            "kind": assertion.kind,
            "semanticKey": assertion.semantic_key,
            "value": super::decode::<Value>(value)?,
            "status": status,
            "evidence": assertion.evidence,
            "instructionAuthority": "none"
        }))
        .map_err(|_| "Personal State candidate could not be encoded".to_string())?;
        if candidates.len() >= 512 {
            return Err("required_context_overflow: required Personal State item count exceeds its safe bound".into());
        }
        let source_version: u64 = c
            .query_row(
                "SELECT COALESCE(MAX(sequence),0)+1 FROM personal_transitions
                 WHERE assertion_id=?1",
                [&assertion.id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let mut candidate = Candidate::untrusted(
            format!("personal-assertion:{}", assertion.id),
            "personal-state",
            assertion_scopes,
            Requirement::Should,
            assertion.id.clone(),
            source_version,
            if status == Status::Active { 700 } else { 900 },
            content,
        );
        // Current state is required context, but remains untrusted data. Keep the central
        // classifier as the source of truth so the broker can also defend direct callers.
        candidate.requirement = crate::runtime::context::required::requirement(&candidate);
        candidates.push(candidate);
    }
    let keys_json = scope.keys_json()?;
    let mut statement = c
        .prepare(
            "SELECT p.sequence,p.message_id
             FROM personal_sources p JOIN personal_jobs j ON j.source_sequence=p.sequence
             WHERE p.available=1 AND p.bytes>0 AND j.status!='completed' AND p.message_id!=?1
               AND (
                 (?3=1 AND NOT EXISTS(
                   SELECT 1 FROM conversation_message_scopes m WHERE m.message_id=p.message_id
                 ))
                 OR EXISTS(
                   SELECT 1 FROM conversation_message_scopes m JOIN json_each(?2) allowed
                     ON allowed.value=m.scope_key WHERE m.message_id=p.message_id
                 )
               )
             ORDER BY p.sequence",
        )
        .map_err(database_error)?;
    let pending = statement
        .query_map(
            params![current_message_id, keys_json, scope.is_user_only()],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut pending_used = 0_usize;
    for (sequence, message_id) in pending {
        let remaining = max_bytes.saturating_sub(pending_used);
        if remaining == 0 {
            return Err("required_context_overflow: required pending Personal State source exceeds its budget".into());
        }
        let chunk = super::sources::load(c, sequence, 0, remaining.min(32_000))?;
        if !chunk.source.finalized {
            return Err("Required pending Personal State source is incomplete".into());
        }
        let content = serde_json::to_string(&json!({
            "pendingSource": chunk.source.key,
            "text": chunk.text,
            "instructionAuthority": "none"
        }))
        .map_err(|_| "Pending Personal State source could not be encoded".to_string())?;
        pending_used = pending_used.saturating_add(content.len());
        if pending_used > max_bytes {
            return Err("required_context_overflow: required pending Personal State source exceeds its budget".into());
        }
        candidates.push(Candidate::untrusted(
            format!("personal-pending:{message_id}"),
            "personal-pending",
            scope.scopes.iter().map(|value| value.key.clone()).collect(),
            Requirement::Must,
            chunk.source.key.id,
            chunk.source.key.version,
            1_000,
            content,
        ));
    }
    Ok(candidates)
}

fn canonical_scope_key(value: &str) -> String {
    if ["user:", "project:", "task:", "resource:", "request:"]
        .iter()
        .any(|prefix| value.starts_with(prefix))
    {
        value.to_string()
    } else {
        format!("request:{value}")
    }
}

/// Required current state and pending originals are never silently truncated.
/// This is JSON data, never a new instruction source.
#[cfg(test)]
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
        if a.kind.is_world() {
            continue;
        }
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
