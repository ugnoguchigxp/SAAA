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

/// Resolve the single authorized knowledge scope of a World-touching patch and verify that
/// every input source is valid and mapped to that project. Returns the scope.
pub(crate) fn validate_scope_and_sources(
    c: &Connection,
    patch: &StatePatch,
    context: &CommitContext<'_>,
) -> Result<String, String> {
    // World work uses an active project or the exact principal user scope from the
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
    if !saaa_personal_state_core::world::validation_v2::is_knowledge_scope(
        &project_scope,
        context.access.principal,
    ) {
        return Err("world-scope-denied".into());
    }
    // The scope must exist and be active; names never mint one.
    let scope_active: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM context_scopes WHERE scope_key=?1 AND state='active' AND kind=?2)",
            params![project_scope, if project_scope.starts_with("user:") { "user" } else { "project" }],
            |r| r.get(0),
        )
        .map_err(database_error)?;
    if !scope_active {
        return Err("world-scope-denied".into());
    }
    // Every input source version must be valid and mapped to this project.
    let retained = retained_relation_dependencies(c, patch, context, &project_scope)?;
    if context.input_dependencies.len() > 128 {
        return Err("world-limit".into());
    }
    let mut source_count = 0usize;
    for key in &context.input_dependencies {
        source_count += usize::from(!retained.contains(key));
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

/// A versioned relation's existing evidence set is authoritative history, rather
/// than another batch of model-supplied sources. Every retained key still passes
/// scope mapping and the reducer's availability/forget checks. No arbitrary
/// reference becomes historical merely because it appears in SQLite.
fn retained_relation_dependencies(
    c: &Connection,
    patch: &StatePatch,
    context: &CommitContext<'_>,
    scope: &str,
) -> Result<std::collections::BTreeSet<SourceKey>, String> {
    let ledger = crate::memory::personal_state::store::load(c)?;
    let mut retained = std::collections::BTreeSet::new();
    for transition in &patch.transitions {
        let Action::Supersede { by } = &transition.action else {
            continue;
        };
        let Some(prior) = ledger.assertions.get(&transition.assertion_id) else {
            continue;
        };
        let Some(next) = patch.assertions.iter().find(|a| &a.id == by) else {
            continue;
        };
        if prior.kind != Kind::WorldRelation
            || next.kind != prior.kind
            || prior.semantic_key != next.semantic_key
            || prior.access.task_request.as_deref() != Some(scope)
            || next.access.task_request.as_deref() != Some(scope)
            || ledger.status(&prior.id, context.now) != Status::Active
            || !prior.input_dependencies.is_subset(&next.input_dependencies)
            || !prior.evidence.is_subset(&next.evidence)
            || load_payload_json(c, &prior.payload_ref)?["schema_version"] != 2
        {
            continue;
        }
        retained.extend(prior.input_dependencies.clone());
    }
    Ok(retained)
}
