use super::contracts::UiData;
use crate::{database_error, now_iso};
use rusqlite::{params, Connection};
use serde_json::json;

pub(crate) const SOURCES: [&str; 4] = [
    "runtime.summary",
    "runtime.runs",
    "runtime.history",
    "larm.status",
];
pub(crate) fn validate_source(source: &str) -> Result<(), String> {
    if SOURCES.contains(&source) {
        Ok(())
    } else {
        Err("Unknown data source".into())
    }
}
pub(crate) fn validate_field(source: &str, field: &str) -> Result<(), String> {
    let fields: &[&str] = match source {
        "runtime.summary" => &["running", "completed", "failed", "total"],
        "runtime.runs" => &["id", "provider", "status", "startedAt"],
        "runtime.history" => &["time", "count"],
        "larm.status" => &["provider", "runtime", "status", "updatedAt"],
        _ => return Err("Unknown data source".into()),
    };
    if fields.contains(&field) {
        Ok(())
    } else {
        Err(format!("Unknown field {field} for {source}"))
    }
}

pub(crate) fn query(
    connection: &Connection,
    conversation: &str,
    source: &str,
) -> Result<UiData, String> {
    validate_source(source)?;
    let rows = match source {
        "runtime.summary" => {
            let row = connection.query_row("SELECT COUNT(*),COALESCE(SUM(status='running'),0),COALESCE(SUM(status='completed'),0),COALESCE(SUM(status='failed'),0) FROM runtime_runs WHERE conversation_id=?1", [conversation], |row| {
                Ok(json!({"total":row.get::<_,i64>(0)?,"running":row.get::<_,i64>(1)?,"completed":row.get::<_,i64>(2)?,"failed":row.get::<_,i64>(3)?}))
            }).map_err(database_error)?;
            vec![row]
        }
        "runtime.runs" => {
            let mut statement = connection.prepare("SELECT id,provider_id,status,started_at FROM runtime_runs WHERE conversation_id=?1 ORDER BY started_at DESC,id DESC LIMIT 100").map_err(database_error)?;
            let rows = statement.query_map([conversation], |row| Ok(json!({"id":row.get::<_,String>(0)?,"provider":row.get::<_,Option<String>>(1)?,"status":row.get::<_,String>(2)?,"startedAt":row.get::<_,String>(3)?}))).map_err(database_error)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?
        }
        "larm.status" => {
            let mut statement = connection.prepare("SELECT p.provider_id,p.selected_runtime_id,p.status,p.updated_at FROM provider_sessions p JOIN runtime_runs r ON r.id=p.runtime_run_id WHERE r.conversation_id=?1 AND p.provider_kind='larm' ORDER BY p.updated_at DESC,p.id DESC LIMIT 100").map_err(database_error)?;
            let rows = statement.query_map([conversation], |row| Ok(json!({"provider":row.get::<_,String>(0)?,"runtime":row.get::<_,Option<String>>(1)?,"status":row.get::<_,String>(2)?,"updatedAt":row.get::<_,String>(3)?}))).map_err(database_error)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?
        }
        _ => {
            let mut statement = connection.prepare("SELECT CAST(started_at AS INTEGER)/60000*60000 AS bucket,COUNT(*) FROM runtime_runs WHERE conversation_id=?1 GROUP BY bucket ORDER BY bucket DESC LIMIT 60").map_err(database_error)?;
            let rows = statement
                .query_map(params![conversation], |row| {
                    Ok(json!({"time":row.get::<_,i64>(0)?,"count":row.get::<_,i64>(1)?}))
                })
                .map_err(database_error)?;
            let mut rows = rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            rows.reverse();
            rows
        }
    };
    Ok(UiData {
        captured_at: now_iso(),
        rows,
    })
}
