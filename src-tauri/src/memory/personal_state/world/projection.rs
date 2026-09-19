//! World projection rebuild (WM-06). Runs inside the Writer transaction.
//!
//! The projection is fully re-derived from the canonical ledger and the
//! erasable payload store. It never updates the canonical payload. A payload
//! that cannot be decoded is an explicit corruption error, not a silent skip.

use crate::database_error;
use rusqlite::{params, Connection};
use saaa_personal_state_core::world::{
    normalize_name, EntityPayload, FocusPayload, FocusReason, RelationPayload, WorldPayload,
};
use saaa_personal_state_core::*;

pub fn rebuild(c: &Connection, ledger: &Ledger, now: i64) -> Result<(), String> {
    // Remove the whole projection (including meta) first so readers see it as
    // stale while rebuilding. Children before parents for foreign keys.
    wipe(c)?;
    // Pass 1: active entities (relation and focus endpoints must exist first).
    for (id, assertion) in &ledger.assertions {
        if assertion.kind != Kind::WorldEntity {
            continue;
        }
        let status = ledger.status(id, now);
        if status != Status::Active {
            continue;
        }
        let Some(WorldPayload::Entity(entity)) = decode(c, assertion)? else {
            continue;
        };
        insert_entity(c, ledger, assertion, &entity, status)?;
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
        let Some(WorldPayload::Relation(relation)) = decode(c, assertion)? else {
            continue;
        };
        insert_relation(c, ledger, assertion, &relation, status)?;
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
        let Some(WorldPayload::Focus(focus)) = decode(c, assertion)? else {
            continue;
        };
        if focus_usable(ledger, &focus, now)? {
            insert_focus(c, ledger, assertion, &focus, status)?;
        }
    }
    c.execute(
        "INSERT OR REPLACE INTO personal_world_projection_meta
           (id,ledger_revision,input_epoch,policy_revision,built_at_ms)
         VALUES(1,?1,?2,?3,?4)",
        params![
            ledger.revision,
            ledger.input_epoch,
            ledger.policy_revision,
            now
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

fn decode(c: &Connection, assertion: &Assertion) -> Result<Option<WorldPayload>, String> {
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
    WorldPayload::decode(kind, &value)
        .map(Some)
        .map_err(|_| "world-projection-corrupt".into())
}

fn insert_entity(
    c: &Connection,
    ledger: &Ledger,
    assertion: &Assertion,
    entity: &EntityPayload,
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
            entity_kind(entity),
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
    relation: &RelationPayload,
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
    focus: &FocusPayload,
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
            focus_reason(focus),
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
fn focus_usable(ledger: &Ledger, focus: &FocusPayload, now: i64) -> Result<bool, String> {
    if focus.reason != FocusReason::CurrentWork {
        return Ok(true);
    }
    let Some(objective) = focus.objective_assertion_id.as_deref() else {
        return Ok(false);
    };
    Ok(ledger.status(objective, now) == Status::Active)
}

fn entity_kind(entity: &EntityPayload) -> &'static str {
    match entity.entity_kind {
        saaa_personal_state_core::world::EntityKind::Project => "project",
        saaa_personal_state_core::world::EntityKind::Concept => "concept",
        saaa_personal_state_core::world::EntityKind::Metric => "metric",
    }
}

fn focus_reason(focus: &FocusPayload) -> &'static str {
    match focus.reason {
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
