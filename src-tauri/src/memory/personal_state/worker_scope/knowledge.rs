//! Resolve registered knowledge Scope without changing continuity semantics.
use super::*;
/// World uses registered knowledge Scope metadata without changing continuity.
/// Task scope resolves only a unique, explicitly mapped active Project.
pub(crate) fn knowledge_request(
    c: &Connection,
    job: &Job,
    source: &saaa_personal_state_core::SourceKey,
) -> Result<Option<String>, String> {
    let Some(scope) = job.scope_key.as_deref() else {
        return Ok(None);
    };
    let principal: String = c
        .query_row("SELECT principal FROM personal_scope", [], |r| r.get(0))
        .map_err(crate::database_error)?;
    if saaa_personal_state_core::world::validation_v2::is_knowledge_scope(scope, &principal) {
        return Ok(Some(scope.into()));
    }
    if !scope.starts_with("task:") {
        return Ok(None);
    }
    let mut q=c.prepare("SELECT DISTINCT r.scope_key FROM personal_source_scope_refs r JOIN context_scopes s ON s.scope_key=r.scope_key JOIN context_scope_links l ON l.parent_scope_key=s.scope_key AND l.child_scope_key=?3 WHERE r.source_id=?1 AND r.version=?2 AND s.kind='project' AND s.state='active' LIMIT 2").map_err(crate::database_error)?;
    let projects = q
        .query_map(rusqlite::params![source.id, source.version, scope], |r| {
            r.get::<_, String>(0)
        })
        .map_err(crate::database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::database_error)?;
    Ok(if projects.len() == 1 {
        projects.into_iter().next()
    } else {
        None
    })
}
