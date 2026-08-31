use rusqlite::{params, Connection};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::*;

pub(super) fn enqueue_job(
    connection: &Connection,
    job_kind: &str,
    source_window_id: Option<&str>,
    now: &str,
) -> Result<(), String> {
    if !TURN_JOB_KINDS.contains(&job_kind) && job_kind != "outbox_delivery" {
        return Err("Unsupported memory job kind".into());
    }
    let seed = format!("{job_kind}:{}", source_window_id.unwrap_or("none"));
    let id = format!("memory_job_{}", &sha256_hex(seed.as_bytes())[..32]);
    connection
        .execute(
            "INSERT INTO memory_reflection_jobs(
               id,job_kind,source_window_id,status,created_at,updated_at
             ) VALUES(?1,?2,?3,'queued',?4,?4)
             ON CONFLICT(job_kind,source_window_id) DO NOTHING",
            params![id, job_kind, source_window_id, now],
        )
        .map_err(database_error)?;
    Ok(())
}

#[derive(Debug)]
pub(super) struct SourceMessage {
    pub(super) ordinal: i64,
    pub(super) conversation_id: String,
    pub(super) task_mode: String,
    pub(super) role: String,
    pub(super) content: String,
    pub(super) created_at: String,
}

pub(super) fn load_source_message(
    connection: &Connection,
    id: &str,
) -> Result<SourceMessage, String> {
    connection
        .query_row(
            "SELECT m.rowid,m.conversation_id,c.task_mode,m.role,m.content,m.created_at
             FROM conversation_messages m
             JOIN conversations c ON c.id=m.conversation_id
             WHERE m.id=?1",
            params![id],
            |row| {
                Ok(SourceMessage {
                    ordinal: row.get(0)?,
                    conversation_id: row.get(1)?,
                    task_mode: row.get(2)?,
                    role: row.get(3)?,
                    content: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| "Memory source message is unavailable".to_string())
}

pub(super) fn load_source_window_by_ref(
    connection: &Connection,
    source_ref: &str,
) -> Result<SourceWindow, String> {
    connection
        .query_row(
            "SELECT id,source_ref,start_message_id,end_message_id,source_digest
             FROM memory_source_windows WHERE source_ref=?1 AND availability='available'",
            params![source_ref],
            |row| {
                Ok(SourceWindow {
                    id: row.get(0)?,
                    source_ref: row.get(1)?,
                    start_message_id: row.get(2)?,
                    end_message_id: row.get(3)?,
                    source_digest: row.get(4)?,
                })
            },
        )
        .map_err(database_error)
}

pub(super) fn digest_source_messages(start: &SourceMessage, end: &SourceMessage) -> String {
    let mut hasher = Sha256::new();
    for message in [start, end] {
        for value in [
            message.conversation_id.as_bytes(),
            message.role.as_bytes(),
            message.created_at.as_bytes(),
            message.content.as_bytes(),
        ] {
            hasher.update(value.len().to_be_bytes());
            hasher.update(value);
        }
    }
    format!("{:x}", hasher.finalize())
}

pub(super) fn require_available_source(
    connection: &Connection,
    source_window_id: &str,
) -> Result<(), String> {
    let available: bool = connection
        .query_row(
            "SELECT EXISTS(
               SELECT 1 FROM memory_source_windows WHERE id=?1 AND availability='available'
             )",
            params![source_window_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !available {
        return Err("Memory source window is unavailable".into());
    }
    Ok(())
}

pub(super) const PROFILE_KINDS: &[&str] = &["preference", "communication", "accessibility"];
pub(super) const WORKING_KINDS: &[&str] =
    &["open_loop", "commitment", "constraint", "pending_decision"];
pub(super) const CAPSULE_KINDS: &[&str] = &[
    "active_referent",
    "constraint",
    "open_loop",
    "commitment",
    "recent_decision",
];

pub(super) fn validate_item(
    item_kind: &str,
    semantic_key: &str,
    value: &Value,
    priority: i64,
    allowed_kinds: &[&str],
) -> Result<(), String> {
    let encoded = serde_json::to_string(value).map_err(|error| error.to_string())?;
    if !allowed_kinds.contains(&item_kind)
        || semantic_key.is_empty()
        || semantic_key.len() > MAX_SEMANTIC_KEY_BYTES
        || encoded.len() > MAX_ITEM_JSON_BYTES
        || !(-100..=100).contains(&priority)
    {
        return Err("Invalid bounded memory item".into());
    }
    Ok(())
}

pub(super) fn validate_optional_timestamp(value: Option<&str>) -> Result<(), String> {
    if value.is_some_and(|value| {
        value.is_empty() || value.len() > 32 || !value.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        return Err("Invalid memory item timestamp".into());
    }
    Ok(())
}

pub(super) fn load_items<P: rusqlite::Params>(
    connection: &Connection,
    sql: &str,
    parameters: P,
    output: &mut Vec<ProjectionItem>,
) -> Result<(), String> {
    let mut statement = connection.prepare_cached(sql).map_err(database_error)?;
    let rows = statement
        .query_map(parameters, |row| {
            let memory_class: String = row.get(0)?;
            let encoded: String = row.get(3)?;
            let value = serde_json::from_str(&encoded).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    3,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok((
                memory_class,
                row.get(1)?,
                row.get(2)?,
                value,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    for (memory_class, item_kind, semantic_key, value, source_ref, priority, valid_until) in rows {
        let memory_class = match memory_class.as_str() {
            "user_core" => "user_core",
            "working_state" => "working_state",
            "continuity_capsule" => "continuity_capsule",
            _ => return Err("Invalid persisted memory projection class".into()),
        };
        output.push(ProjectionItem {
            memory_class,
            item_kind,
            semantic_key,
            value,
            source_ref,
            priority,
            valid_until,
        });
    }
    Ok(())
}

pub(super) fn trim_observability_events(
    connection: &Connection,
    table: &str,
) -> Result<(), String> {
    let sql = match table {
        "memory_decision_events" => {
            "DELETE FROM memory_decision_events WHERE id IN (
               SELECT id FROM memory_decision_events
               ORDER BY CAST(created_at AS INTEGER) DESC,id DESC
               LIMIT -1 OFFSET ?1
             )"
        }
        "context_projection_events" => {
            "DELETE FROM context_projection_events WHERE id IN (
               SELECT id FROM context_projection_events
               ORDER BY CAST(created_at AS INTEGER) DESC,id DESC
               LIMIT -1 OFFSET ?1
             )"
        }
        _ => return Err("Invalid observability event table".into()),
    };
    connection
        .execute(sql, params![MAX_OBSERVABILITY_EVENTS])
        .map_err(database_error)?;
    Ok(())
}

pub(super) fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub(super) fn database_error(error: rusqlite::Error) -> String {
    format!("Memory control-plane SQLite operation failed: {error}")
}
