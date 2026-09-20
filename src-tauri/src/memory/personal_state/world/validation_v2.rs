//! Shared-commit World validation hook, versioned path (D16/D33).
//!
//! v1 and v2 payloads are decoded with the versioned decoder, v2 references are
//! content-addressed, and the version-aware duplicate/capacity checks run even
//! for a v1-only patch. The v1 semantic pass is preserved for v1-only patches.

use super::validation::{kind_name, load_payload_json, validate_scope_and_sources};
use rusqlite::Connection;
use saaa_personal_state_core::world::validation_v2::{
    validate_versioned_patch, VersionedPatchInput,
};
use saaa_personal_state_core::world::versioned::VersionedWorldPayload;
use saaa_personal_state_core::world::versioned::{decode_versioned, verify_payload_ref};
use saaa_personal_state_core::world::{validate_world_patch, WorldPatchInput, WorldPayload};
use saaa_personal_state_core::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resend {
    New,
    Applied,
    Conflict,
}

/// C5/D17: request authorization plus re-send/no-op detection, before Source
/// re-validation and World semantics.
pub fn resend(
    ledger: &Ledger,
    patch: &StatePatch,
    context: &CommitContext<'_>,
) -> Result<Resend, String> {
    if !context.access.authorized
        || context.access.principal != ledger.principal
        || context.access.scope != ledger.scope
    {
        return Err("personal-patch-Unauthorized".into());
    }
    let Some(prior) = ledger.applied_patches.get(&patch.id) else {
        return Ok(Resend::New);
    };
    let fingerprint = patch_fingerprint(patch).map_err(|e| format!("personal-patch-{e}"))?;
    Ok(if prior == &fingerprint {
        Resend::Applied
    } else {
        Resend::Conflict
    })
}

/// Decode every existing World assertion payload through the versioned decoder.
/// v1 and v2 coexist; an unknown version or malformed payload is corruption.
pub fn load_existing_versioned(
    c: &Connection,
    ledger: &Ledger,
) -> Result<BTreeMap<String, VersionedWorldPayload>, String> {
    let mut existing = BTreeMap::new();
    for assertion in ledger.assertions.values() {
        if !assertion.kind.is_world() {
            continue;
        }
        let raw = load_payload_json(c, &assertion.payload_ref)?;
        let payload = decode_versioned(kind_name(assertion.kind), &raw)
            .map_err(|_| "world-projection-corrupt".to_string())?;
        existing.insert(assertion.payload_ref.clone(), payload);
    }
    Ok(existing)
}

/// Verify that every new v2 World payload is content-addressed (C5). This is DB
/// free and must run before a re-send can be reported as a no-op, so a swapped
/// outer payload map is rejected rather than silently accepted.
pub fn verify_world_payload_refs(
    patch: &StatePatch,
    payloads: &BTreeMap<String, serde_json::Value>,
) -> Result<(), String> {
    for assertion in &patch.assertions {
        if !assertion.kind.is_world() {
            continue;
        }
        let Some(value) = payloads.get(&assertion.payload_ref) else {
            return Err("world-invalid-payload".into());
        };
        let payload = decode_versioned(kind_name(assertion.kind), value)
            .map_err(|_| "world-invalid-payload".to_string())?;
        if !verify_payload_ref(&assertion.payload_ref, &payload) {
            return Err("world-invalid-payload".into());
        }
    }
    Ok(())
}

fn touches_world(ledger: &Ledger, patch: &StatePatch) -> bool {
    patch.assertions.iter().any(|a| a.kind.is_world())
        || patch.transitions.iter().any(|t| {
            ledger
                .assertions
                .get(&t.assertion_id)
                .is_some_and(|a| a.kind.is_world())
        })
}

/// Versioned validation. Runs for v1-only patches too, then preserves the v1
/// semantic pass. `store::commit` calls this.
pub fn validate_commit_v2(
    c: &Connection,
    ledger: &Ledger,
    patch: &StatePatch,
    context: &CommitContext<'_>,
    payloads: &BTreeMap<String, serde_json::Value>,
) -> Result<(), String> {
    if !touches_world(ledger, patch) {
        return Ok(());
    }
    let project_scope = validate_scope_and_sources(c, patch, context)?;
    let mut new_payloads: BTreeMap<String, VersionedWorldPayload> = BTreeMap::new();
    for assertion in &patch.assertions {
        if !assertion.kind.is_world() {
            continue;
        }
        let value = payloads
            .get(&assertion.payload_ref)
            .ok_or("world-invalid-payload")?;
        let payload = decode_versioned(kind_name(assertion.kind), value)
            .map_err(|_| "world-invalid-payload".to_string())?;
        // A v2 payload reference must be the hash of its canonical bytes. This
        // is what makes a swapped outer payload map detectable.
        if !verify_payload_ref(&assertion.payload_ref, &payload) {
            return Err("world-invalid-payload".into());
        }
        new_payloads.insert(assertion.payload_ref.clone(), payload);
    }
    let existing = load_existing_versioned(c, ledger)?;
    validate_versioned_patch(&VersionedPatchInput {
        ledger,
        patch,
        payloads: &new_payloads,
        existing: &existing,
        project_scope: &project_scope,
        now: context.now,
    })
    .map_err(|e| e.code().to_string())?;

    // Preserve the v1 semantic pass for v1-only payloads. Mixed patches are
    // fully covered by the versioned validator.
    let has_v2 = new_payloads
        .values()
        .any(|p| matches!(p, VersionedWorldPayload::V2(_)));
    if !has_v2 {
        let mut v1_new: BTreeMap<String, WorldPayload> = BTreeMap::new();
        for (reference, payload) in &new_payloads {
            if let VersionedWorldPayload::V1(p) = payload {
                v1_new.insert(reference.clone(), p.clone());
            }
        }
        let mut v1_existing: BTreeMap<String, WorldPayload> = BTreeMap::new();
        for (reference, payload) in &existing {
            if let VersionedWorldPayload::V1(p) = payload {
                v1_existing.insert(reference.clone(), p.clone());
            }
        }
        validate_world_patch(&WorldPatchInput {
            ledger,
            patch,
            payloads: &v1_new,
            existing: &v1_existing,
            project_scope: &project_scope,
            now: context.now,
        })
        .map_err(|e| e.code().to_string())?;
    }
    Ok(())
}
