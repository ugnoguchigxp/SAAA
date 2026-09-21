//! Legacy metadata path when memory is disabled.
use super::*;
pub(super) fn snapshot(c: &Connection, run_id: &str) -> Result<Value, String> {
    let scope = scope::load(c, run_id)?;
    if scope.status != "resolved" {
        return Err("Codex scope is unresolved".into());
    }
    let sources = inputs::read(c, &scope)?;
    let policy: u64 = c
        .query_row(
            "SELECT policy_revision FROM personal_scope WHERE id='primary'",
            [],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    Ok(
        json!({"schema":"saaa.codex-world-snapshot.v1","scopeDigest":scope.digest,"policyRevision":policy,
        "focusScope":scope.focus_scope_key,"scopes":scope.scopes.iter().map(|s| json!({"key":s.key,"kind":s.kind,"relation":s.relation,"epoch":s.epoch})).collect::<Vec<_>>(),"sources":sources}),
    )
}
pub(super) fn unchanged(c: &Connection, run_id: &str, prior: &Value) -> Result<(), String> {
    let mut current = snapshot(c, run_id)?;
    // Observation time changes on every read; owner fields and revisions must not change.
    current["sources"]["observedAt"] = prior["sources"]["observedAt"].clone();
    if current != *prior {
        return Err("Codex World source changed; retry with current context".into());
    }
    Ok(())
}
