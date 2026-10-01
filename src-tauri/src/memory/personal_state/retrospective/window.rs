use super::*;
pub(super) struct Window {
    pub primary: sources::Chunk,
    pub context: Vec<sources::Chunk>,
    pub end: u64,
    pub historical: bool,
    pub incomplete: bool,
}
pub(super) fn load(c: &Connection, j: &jobs::ReviewJob) -> Result<Window, String> {
    let principal: String = c
        .query_row("SELECT principal FROM personal_scope", [], |r| r.get(0))
        .map_err(database_error)?;
    if !saaa_personal_state_core::world::validation_v2::is_knowledge_scope(&j.scope, &principal) {
        return Err("world-review-scope".into());
    }
    let mut q=c.prepare("SELECT p.sequence FROM personal_sources p JOIN personal_source_scope_refs r ON r.source_id=p.message_id AND r.version=p.version WHERE p.available=1 AND p.role IN ('user','transcript') AND r.scope_key=?1 AND p.sequence>=?2 AND p.sequence<=?3 ORDER BY p.sequence LIMIT 12").map_err(database_error)?;
    let seqs = q
        .query_map(params![j.scope, j.sequence, j.boundary], |r| {
            r.get::<_, u64>(0)
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let first = *seqs.first().ok_or("personal-source-unavailable")?;
    let mut chunks = Vec::new();
    let mut remaining = 32000usize;
    let mut incomplete = false;
    let mut end = first;
    for seq in seqs.iter().take(4) {
        let chunk = sources::load(c, *seq, 0, 32000)?;
        if !chunk.source.finalized || chunk.text.len() > remaining {
            incomplete = true;
            if chunks.is_empty() {
                chunks.push(chunk);
                end = *seq;
            }
            break;
        }
        remaining -= chunk.text.len();
        end = *seq;
        chunks.push(chunk);
    }
    // The latest included owner statement supplies the primary quote and its actual time.
    // Earlier statements remain quoted context, never independent model-created evidence.
    let primary = chunks.pop().ok_or("personal-source-unavailable")?;
    let context = if j.current {
        sources::world_context(c, &primary.source, &j.scope, primary.text.len())?
    } else {
        chunks
    };
    let historical: bool=c.query_row("SELECT EXISTS(SELECT 1 FROM personal_sources p JOIN personal_source_scope_refs r ON r.source_id=p.message_id AND r.version=p.version WHERE r.scope_key=?1 AND p.available=1 AND p.role IN ('user','transcript') AND p.sequence>?2)", params![j.scope,end], |r|r.get(0)).map_err(database_error)?;
    Ok(Window {
        primary,
        context,
        end,
        historical,
        incomplete,
    })
}
pub(super) fn current(c: &Connection, scope: &str, text: &str) -> Result<Value, String> {
    let ledger = store::load(c)?;
    let mut values = Vec::new();
    let mut budget = 10000usize;
    // Stable bounded subset, with explicit incomplete context rather than a full ledger dump.
    for a in ledger.assertions.values().filter(|a| {
        (a.access.task_request.as_deref() == Some(scope)
            || (scope == format!("user:{}", ledger.principal)
                && a.kind == saaa_personal_state_core::Kind::Objective
                && a.access.task_request.is_none()
                && a.access.principal == ledger.principal))
            && (a.kind.is_world() || a.kind == saaa_personal_state_core::Kind::Objective)
            && ledger.status(&a.id, super::super::now()) == saaa_personal_state_core::Status::Active
    }) {
        if a.kind != saaa_personal_state_core::Kind::Objective
            && !text.contains(&a.semantic_key)
            && values.len() >= 8
        {
            continue;
        }
        let payload = super::super::world::validation::load_payload_json(c, &a.payload_ref)?;
        let value = json!({"id":a.id,"kind":a.kind,"payload":payload});
        let bytes = serde_json::to_vec(&value)
            .map_err(|_| "world-review-json")?
            .len();
        if bytes > budget {
            continue;
        }
        budget -= bytes;
        values.push(value);
    }
    Ok(Value::Array(values))
}

pub(super) fn revalidate_current(c: &Connection, current: &Value) -> Result<(), String> {
    let ledger = store::load(c)?;
    for entry in current.as_array().into_iter().flatten() {
        let id = entry["id"].as_str().ok_or("world-review-premise")?;
        let a = ledger.assertions.get(id).ok_or("world-review-premise")?;
        if ledger.status(id, super::super::now()) != saaa_personal_state_core::Status::Active
            || super::super::world::validation::load_payload_json(c, &a.payload_ref)?
                != entry["payload"]
        {
            return Err("world-review-premise".into());
        }
    }
    Ok(())
}

pub(super) fn source_labels(manifest: Option<&str>) -> Result<Vec<Value>, String> {
    let manifest: Value =
        serde_json::from_str(manifest.unwrap_or("[]")).map_err(|_| "world-review-json")?;
    Ok(manifest[0].as_array().into_iter().flatten().map(|r|json!({"id":r["key"]["id"],"version":r["key"]["version"],"observedAt":r["recorded_at"]})).collect())
}
