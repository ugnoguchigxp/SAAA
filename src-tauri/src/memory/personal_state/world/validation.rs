//! Shared-commit World validation hook, v1 path (WM-07).
//!
//! This runs inside the caller's Writer transaction, before `Ledger::apply`, so
//! existing `store::commit` cannot bypass World semantics. It performs no
//! network or await. The versioned path lives in `validation_v2.rs`.

use crate::database_error;
use rusqlite::{params, Connection};
use saaa_personal_state_core::*;

pub(crate) fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::WorldEntity => "world_entity",
        Kind::WorldRelation => "world_relation",
        Kind::WorldFocus => "world_focus",
        _ => "",
    }
}

pub(crate) fn load_payload_json(
    c: &Connection,
    payload_ref: &str,
) -> Result<serde_json::Value, String> {
    let raw: String = c
        .query_row(
            "SELECT value_json FROM personal_payloads WHERE id=?1",
            [payload_ref],
            |r| r.get(0),
        )
        .map_err(|_| "world-projection-corrupt")?;
    serde_json::from_str(&raw).map_err(|_| "world-projection-corrupt".into())
}

/// Resolve the single project scope of a World-touching patch and verify that
/// every input source is valid and mapped to that project. Returns the scope.
pub(crate) fn validate_scope_and_sources(
    c: &Connection,
    patch: &StatePatch,
    context: &CommitContext<'_>,
) -> Result<String, String> {
    // All World work is project-scoped; the project scope comes from the
    // trusted adapter context, never from payload text.
    let project_scope = patch
        .assertions
        .iter()
        .filter(|a| a.kind.is_world())
        .map(|a| a.access.task_request.clone())
        .chain(std::iter::once(
            context.access.task_request.map(str::to_string),
        ))
        .flatten()
        .collect::<std::collections::BTreeSet<_>>();
    if project_scope.len() != 1 {
        return Err("world-scope-denied".into());
    }
    let project_scope = project_scope.into_iter().next().expect("one scope");
    if !project_scope.starts_with("project:") {
        return Err("world-scope-denied".into());
    }
    // The project scope must be an existing active scope; names never mint one.
    let scope_active: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM context_scopes WHERE scope_key=?1 AND state='active')",
            [&project_scope],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if !scope_active {
        return Err("world-scope-denied".into());
    }
    // Every input source version must be valid and mapped to this project.
    let mut source_count = 0usize;
    for key in &context.input_dependencies {
        source_count += 1;
        let mapped: bool = c
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM personal_source_scope_refs
                   WHERE source_id=?1 AND version=?2 AND scope_key=?3)",
                params![key.id, key.version, project_scope],
                |r| r.get(0),
            )
            .map_err(database_error)?;
        if !mapped {
            return Err("world-scope-denied".into());
        }
    }
    if source_count > saaa_personal_state_core::world::MAX_WORLD_SOURCES {
        return Err("world-limit".into());
    }
    Ok(project_scope)
}
