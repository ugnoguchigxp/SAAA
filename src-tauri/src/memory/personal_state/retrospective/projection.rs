use super::window::source_labels;
use rusqlite::Connection;
use serde_json::Value;
pub(super) fn candidates(c: &Connection) -> Result<Value, String> {
    let mut q=c.prepare("SELECT w.id,w.scope_key,w.result,w.proposal,w.manifest FROM personal_review_work w JOIN context_scopes s ON s.scope_key=w.scope_key AND s.state='active' WHERE w.status IN ('preview','held') AND w.proposal IS NOT NULL AND NOT EXISTS(SELECT 1 FROM personal_review_inputs i LEFT JOIN personal_sources p ON p.message_id=i.source_id AND p.version=i.version AND p.available=1 LEFT JOIN personal_source_scope_refs r ON r.source_id=i.source_id AND r.version=i.version AND r.scope_key=w.scope_key WHERE i.work_id=w.id AND (p.sequence IS NULL OR r.source_id IS NULL)) ORDER BY w.id DESC LIMIT 32").map_err(crate::database_error)?;
    let rows = q
        .query_map([], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(crate::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
    let mut result = Vec::new();
    for (id, scope, reason, proposal, manifest) in rows {
        let proposal: Value = serde_json::from_str(&proposal).map_err(|_| "world-review-json")?;
        let sources = source_labels(manifest.as_deref())?;
        result.push(serde_json::json!({"id":id,"scope":scope,"reason":reason,"candidates":proposal["candidates"],"sources":sources}));
    }
    Ok(Value::Array(result))
}
