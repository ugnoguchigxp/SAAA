//! Shared-commit World validation hook (WM-07).
//!
//! This runs inside the caller's Writer transaction, before `Ledger::apply`, so
//! existing `store::commit` cannot bypass World semantics. It performs no
//! network or await.

use crate::database_error;
use rusqlite::{params, Connection};
use saaa_personal_state_core::world::{validate_world_patch, WorldPatchInput, WorldPayload};
use saaa_personal_state_core::*;
use std::collections::BTreeMap;

/// Decode every existing World assertion payload into typed values. A World
/// assertion whose payload cannot be decoded is an explicit corruption error.
pub fn load_existing(
    c: &Connection,
    ledger: &Ledger,
) -> Result<BTreeMap<String, WorldPayload>, String> {
    let mut existing = BTreeMap::new();
    for assertion in ledger.assertions.values() {
        if !assertion.kind.is_world() {
            continue;
        }
        let raw: String = c
            .query_row(
                "SELECT value_json FROM personal_payloads WHERE id=?1",
                [&assertion.payload_ref],
                |r| r.get(0),
            )
            .map_err(|_| "world-projection-corrupt")?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| "world-projection-corrupt")?;
        let kind_name = kind_name(assertion.kind);
        let payload = WorldPayload::decode(kind_name, &value)
            .map_err(|_| "world-projection-corrupt".to_string())?;
        existing.insert(assertion.payload_ref.clone(), payload);
    }
    Ok(existing)
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::WorldEntity => "world_entity",
        Kind::WorldRelation => "world_relation",
        Kind::WorldFocus => "world_focus",
        _ => "",
    }
}

/// Validate the patch against the current ledger. `payloads` holds the new
/// payloads keyed by `payload_ref`.
pub fn validate_commit(
    c: &Connection,
    ledger: &Ledger,
    patch: &StatePatch,
    context: &CommitContext<'_>,
    payloads: &BTreeMap<String, serde_json::Value>,
) -> Result<(), String> {
    let touches_world = patch.assertions.iter().any(|a| a.kind.is_world())
        || patch.transitions.iter().any(|t| {
            ledger
                .assertions
                .get(&t.assertion_id)
                .is_some_and(|a| a.kind.is_world())
        });
    if !touches_world {
        return Ok(());
    }
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
    let mut new_payloads: BTreeMap<String, WorldPayload> = BTreeMap::new();
    for assertion in &patch.assertions {
        if !assertion.kind.is_world() {
            continue;
        }
        let value = payloads
            .get(&assertion.payload_ref)
            .ok_or("world-invalid-payload")?;
        let payload = WorldPayload::decode(kind_name(assertion.kind), value)?;
        new_payloads.insert(assertion.payload_ref.clone(), payload);
    }
    let existing = load_existing(c, ledger)?;
    validate_world_patch(&WorldPatchInput {
        ledger,
        patch,
        payloads: &new_payloads,
        existing: &existing,
        project_scope: &project_scope,
        now: context.now,
    })
    .map_err(|e| e.code().to_string())
}
