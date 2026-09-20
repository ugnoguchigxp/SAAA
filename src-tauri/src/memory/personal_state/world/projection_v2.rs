//! Versioned World projection rebuild (D19/D33). Runs inside the Writer
//! transaction.
//!
//! The projection is fully re-derived from the canonical ledger and the
//! erasable payload store. It never updates the canonical payload. A payload
//! that cannot be decoded is an explicit corruption error, not a silent skip.
//! v1 and v2 payloads are normalized to the v2 view before projection; unknown
//! kinds are never silently downgraded.

use crate::database_error;
use rusqlite::{params, Connection};
use saaa_personal_state_core::world::model_v2::{EntityKindV2, RelationTypeV2};
use saaa_personal_state_core::world::versioned::{
    decode_versioned, VersionedWorldPayload, WorldView,
};
use saaa_personal_state_core::world::{normalize_name, FocusReason};
use saaa_personal_state_core::*;

/// Project the ledger. `projection_version` records 2 as soon as any v2
/// payload is projected, so a v1-only reader refuses to misread new kinds.
pub fn rebuild_v2(c: &Connection, ledger: &Ledger, now: i64) -> Result<(), String> {
    // Remove the whole projection (including meta) first so readers see it as
    // stale while rebuilding. Children before parents for foreign keys.
    wipe(c)?;
    let mut saw_v2 = false;
    // Pass 1: active entities (relation and focus endpoints must exist first).
    for (id, assertion) in &ledger.assertions {
        if assertion.kind != Kind::WorldEntity {
            continue;
        }
        let status = ledger.status(id, now);
        if status != Status::Active {
            continue;
        }
        let Some(view) = decode(c, assertion)? else {
            continue;
        };
        let WorldView::Entity(entity) = view else {
            continue;
        };
        if entity.schema_version == 2 {
            saw_v2 = true;
        }
        insert_entity(c, ledger, assertion, &entity.payload, status)?;
    }
    // Pass 2: usable relations (Active and Disputed).
    for (id, assertion) in &ledger.assertions {
        if assertion.kind != Kind::WorldRelation {
            continue;
        }
        let status = ledger.status(id, now);
        if !matches!(status, Status::Active | Status::Disputed) {
            continue;
        }
        let Some(view) = decode(c, assertion)? else {
            continue;
        };
        let WorldView::Relation(relation) = view else {
            continue;
        };
        if relation.schema_version == 2 {
            saw_v2 = true;
        }
        insert_relation(c, ledger, assertion, &relation.payload, status)?;
    }
    // Pass 3: active focus.
    for (id, assertion) in &ledger.assertions {
        if assertion.kind != Kind::WorldFocus {
            continue;
        }
        let status = ledger.status(id, now);
        if status != Status::Active {
            continue;
        }
        let Some(view) = decode(c, assertion)? else {
            continue;
        };
        let WorldView::Focus(focus) = view else {
            continue;
        };
        if focus.schema_version == 2 {
            saw_v2 = true;
        }
        if focus_usable(
            ledger,
            focus.payload.objective_assertion_id.as_deref(),
            focus.payload.reason,
            now,
        )? {
            insert_focus(c, ledger, assertion, &focus.payload, status)?;
        }
    }
    let projection_version: i64 = if saw_v2 { 2 } else { 1 };
    c.execute(
        "INSERT OR REPLACE INTO personal_world_projection_meta
           (id,ledger_revision,input_epoch,policy_revision,built_at_ms,projection_version)
         VALUES(1,?1,?2,?3,?4,?5)",
        params![
            ledger.revision,
            ledger.input_epoch,
            ledger.policy_revision,
            now,
            projection_version
        ],
    )
    .map_err(database_error)?;
    Ok(())
}

pub fn wipe(c: &Connection) -> Result<(), String> {
    c.execute_batch(
        "DELETE FROM personal_world_aliases;
         DELETE FROM personal_world_relations;
         DELETE FROM personal_world_focus;
         DELETE FROM personal_world_entities;
         DELETE FROM personal_world_projection_meta;",
    )
    .map_err(database_error)
}

fn decode(c: &Connection, assertion: &Assertion) -> Result<Option<WorldView>, String> {
    let raw: String = c
        .query_row(
            "SELECT value_json FROM personal_payloads WHERE id=?1",
            [&assertion.payload_ref],
            |r| r.get(0),
        )
        .map_err(|_| "world-projection-corrupt")?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| "world-projection-corrupt")?;
    let kind = match assertion.kind {
        Kind::WorldEntity => "world_entity",
        Kind::WorldRelation => "world_relation",
        Kind::WorldFocus => "world_focus",
        _ => return Ok(None),
    };
    let payload: VersionedWorldPayload =
        decode_versioned(kind, &value).map_err(|_| "world-projection-corrupt".to_string())?;
    Ok(Some(payload.view(&assertion.semantic_key)))
}

fn insert_entity(
    c: &Connection,
    ledger: &Ledger,
    assertion: &Assertion,
    entity: &saaa_personal_state_core::world::model_v2::EntityPayloadV2,
    status: Status,
) -> Result<(), String> {
    let project_scope = assertion
        .access
        .task_request
        .as_deref()
        .ok_or("world-projection-corrupt")?;
    c.execute(
        "INSERT OR REPLACE INTO personal_world_entities
           (assertion_id,project_scope,entity_id,kind,name,canonical_name,status,valid_from,valid_until,revision)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            assertion.id,
            project_scope,
            entity.entity_id,
            entity_kind(entity.entity_kind),
            entity.name,
            normalize_name(&entity.name),
            status_name(status),
            assertion.valid_from,
            assertion.valid_until,
            ledger.revision
        ],
    )
    .map_err(database_error)?;
    for alias in &entity.aliases {
        c.execute(
            "INSERT OR REPLACE INTO personal_world_aliases
               (assertion_id,canonical_alias,project_scope,entity_id)
             VALUES(?1,?2,?3,?4)",
            params![
                assertion.id,
                normalize_name(alias),
                project_scope,
                entity.entity_id
            ],
        )
        .map_err(database_error)?;
    }
    Ok(())
}

fn insert_relation(
    c: &Connection,
    ledger: &Ledger,
    assertion: &Assertion,
    relation: &saaa_personal_state_core::world::model_v2::RelationPayloadV2,
    status: Status,
) -> Result<(), String> {
    let project_scope = assertion
        .access
        .task_request
        .as_deref()
        .ok_or("world-projection-corrupt")?;
    c.execute(
        "INSERT OR REPLACE INTO personal_world_relations
           (assertion_id,project_scope,from_entity_id,to_entity_id,relation_type,status,valid_from,valid_until,revision)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            assertion.id,
            project_scope,
            relation.from_entity_id,
            relation.to_entity_id,
            relation.relation_type.as_str(),
            status_name(status),
            assertion.valid_from,
            assertion.valid_until,
            ledger.revision
        ],
    )
    .map_err(database_error)?;
    Ok(())
}

fn insert_focus(
    c: &Connection,
    ledger: &Ledger,
    assertion: &Assertion,
    focus: &saaa_personal_state_core::world::model_v2::FocusPayloadV2,
    status: Status,
) -> Result<(), String> {
    let project_scope = assertion
        .access
        .task_request
        .as_deref()
        .ok_or("world-projection-corrupt")?;
    c.execute(
        "INSERT OR REPLACE INTO personal_world_focus
           (assertion_id,project_scope,entity_id,reason,objective_assertion_id,status,valid_from,valid_until,revision)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            assertion.id,
            project_scope,
            focus.entity_id,
            focus_reason(focus.reason),
            focus.objective_assertion_id,
            status_name(status),
            assertion.valid_from,
            assertion.valid_until,
            ledger.revision
        ],
    )
    .map_err(database_error)?;
    Ok(())
}

/// A `current_work` Focus whose Objective is no longer Active is not usable.
fn focus_usable(
    ledger: &Ledger,
    objective_assertion_id: Option<&str>,
    reason: FocusReason,
    now: i64,
) -> Result<bool, String> {
    if reason != FocusReason::CurrentWork {
        return Ok(true);
    }
    let Some(objective) = objective_assertion_id else {
        return Ok(false);
    };
    Ok(ledger.status(objective, now) == Status::Active)
}

fn entity_kind(kind: EntityKindV2) -> &'static str {
    match kind {
        EntityKindV2::Project => "project",
        EntityKindV2::Concept => "concept",
        EntityKindV2::Metric => "metric",
        EntityKindV2::Goal => "goal",
        EntityKindV2::Actor => "actor",
    }
}

#[allow(dead_code)]
fn relation_type(relation_type: RelationTypeV2) -> &'static str {
    relation_type.as_str()
}

fn focus_reason(reason: FocusReason) -> &'static str {
    match reason {
        FocusReason::CurrentWork => "current_work",
        FocusReason::ExplicitInterest => "explicit_interest",
    }
}

fn status_name(status: Status) -> &'static str {
    match status {
        Status::Candidate => "candidate",
        Status::Active => "active",
        Status::Disputed => "disputed",
        _ => "inactive",
    }
}
