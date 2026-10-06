//! Metadata-only route evidence in the existing audit ledger. No prompt, key or audio.
use super::{Purpose, ResolvedRoute};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub(crate) fn attributes(route: &ResolvedRoute, attempt: Option<&str>) -> Value {
    json!({"purpose":route.purpose.id(),"connectionId":route.connection_id,"resourceId":route.resource_id,"connectionLabel":route.connection_label,
        "model":route.model,"fingerprint":route.fingerprint,"attemptId":attempt,
        "location":route.location,"selection":route.selection})
}

pub(crate) fn accepted(db: &Connection, job: &str, route: &ResolvedRoute) -> Result<(), String> {
    db.execute("INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,correlation_id,attributes_json)
        VALUES(?1,?2,'provider','purpose-route-accepted','terminal','success',?3,?4)",params![
        format!("audit_{}",uuid::Uuid::new_v4().simple()),crate::now_iso(),job,attributes(route,None).to_string()])
        .map_err(crate::database_error)?;
    Ok(())
}

/// Submission evidence for media consumers, separately from their durable job ledger.
pub(crate) fn attempt(
    db: &Connection,
    job: &str,
    route: &ResolvedRoute,
    id: &str,
    success: Option<bool>,
) -> Result<(), String> {
    db.execute("INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,correlation_id,attributes_json) VALUES(?1,?2,'provider','purpose-route-attempt',?3,?4,?5,?6)",params![
        format!("audit_{}",uuid::Uuid::new_v4().simple()),crate::now_iso(),if success.is_some(){"terminal"}else{"start"},success.map(|ok|if ok{"success"}else{"failure"}),job,attributes(route,Some(id)).to_string()]).map_err(crate::database_error)?;
    Ok(())
}

pub(crate) fn latest(
    db: &Connection,
    snapshot: &super::RegistrySnapshot,
) -> Result<Vec<Value>, String> {
    let mut stmt=db.prepare("SELECT occurred_at,correlation_id,event_name,phase,outcome,attributes_json
        FROM audit_events WHERE event_name IN ('purpose-route-attempt','purpose-route-accepted') ORDER BY sequence DESC LIMIT 256").map_err(crate::database_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(crate::database_error)?;
    let mut values = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let (at, job, event, phase, outcome, raw) = row.map_err(crate::database_error)?;
        let mut value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let purpose = value["purpose"].as_str().unwrap_or_default().to_string();
        if !seen.insert(purpose.clone()) {
            continue;
        }
        let resource_id = value["resourceId"].as_str().unwrap_or_default();
        value["matchesCurrentSettings"] =
            json!(snapshot
                .bindings
                .iter()
                .find(|b| b.purpose.id() == purpose)
                .filter(|b| b.primary_resource_id.as_deref() == Some(resource_id)
                    || b.fallback_resource_ids.iter().any(|id| id == resource_id))
                .and_then(|b| super::resolve_resource(snapshot, b.purpose, resource_id).ok())
                .is_some_and(
                    |route| value["fingerprint"].as_str() == Some(route.fingerprint.as_str())
                ));
        value["occurredAt"] = json!(at);
        value["jobId"] = json!(job);
        value["status"] = json!(if event == "purpose-route-accepted" {
            "accepted"
        } else if phase == "start" {
            "sending"
        } else if outcome.as_deref() == Some("success") {
            "inference-completed"
        } else {
            "failed"
        });
        values.push(value);
        if values.len()
            == [
                Purpose::ConversationRespond,
                Purpose::VoiceTranscribe,
                Purpose::VoiceSpeak,
                Purpose::MediaImageGenerate,
                Purpose::MediaMusicGenerate,
            ]
            .len()
        {
            break;
        }
    }
    Ok(values)
}
