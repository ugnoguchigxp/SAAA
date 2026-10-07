//! Metadata-only route evidence. No prompt, key, or media bytes.
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::{database_error, ResolvedRoute};

pub fn attributes(route: &ResolvedRoute, attempt: Option<&str>) -> Value {
    json!({
        "purpose": route.purpose.id(),
        "connectionId": route.connection_id,
        "resourceId": route.resource_id,
        "connectionLabel": route.connection_label,
        "model": route.model,
        "fingerprint": route.fingerprint,
        "attemptId": attempt,
        "location": route.location,
        "selection": route.selection
    })
}

pub fn accepted(
    db: &Connection,
    job: &str,
    route: &ResolvedRoute,
    now: &str,
) -> Result<(), String> {
    db.execute(
        "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,correlation_id,attributes_json)
        VALUES(?1,?2,'provider','purpose-route-accepted','terminal','success',?3,?4)",
        params![
            format!("audit_{}", uuid::Uuid::new_v4().simple()),
            now,
            job,
            attributes(route, None).to_string()
        ],
    )
    .map_err(database_error)?;
    Ok(())
}

pub fn attempt(
    db: &Connection,
    job: &str,
    route: &ResolvedRoute,
    id: &str,
    success: Option<bool>,
    now: &str,
) -> Result<(), String> {
    db.execute(
        "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,correlation_id,attributes_json) VALUES(?1,?2,'provider','purpose-route-attempt',?3,?4,?5,?6)",
        params![
            format!("audit_{}", uuid::Uuid::new_v4().simple()),
            now,
            if success.is_some() { "terminal" } else { "start" },
            success.map(|ok| if ok { "success" } else { "failure" }),
            job,
            attributes(route, Some(id)).to_string()
        ],
    )
    .map_err(database_error)?;
    Ok(())
}
