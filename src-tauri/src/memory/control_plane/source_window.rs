use super::items::*;
use super::*;
use rusqlite::OptionalExtension;
#[cfg(test)]
use rusqlite::Transaction;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::{collections::HashSet, env};
// Version 2 adds the trusted World-model reading policy to the conversation system template
// (G1/C6). The policy text is static; no data is ever promoted into it.
pub const CONTEXT_POLICY_VERSION: i64 = 2;
// `user_version` is the application database version. Version 15 replaces
// encrypted voice-profile storage with private, local plaintext storage.
#[cfg(test)]
pub(crate) const MAX_ITEM_JSON_BYTES: usize = 4_000;
#[cfg(test)]
pub(crate) const MAX_SEMANTIC_KEY_BYTES: usize = 128;
pub(crate) const MAX_OBSERVABILITY_EVENTS: usize = 10_000;
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceWindow {
    pub id: String,
    pub source_ref: String,
    pub start_message_id: String,
    pub end_message_id: String,
    pub source_digest: String,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionItem {
    pub memory_class: &'static str,
    pub item_kind: String,
    pub semantic_key: String,
    pub value: Value,
    pub source_ref: String,
    pub priority: i64,
    pub valid_until: Option<String>,
}
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapsuleItemInput {
    pub item_kind: String,
    pub semantic_key: String,
    pub value_json: Value,
    pub priority: i64,
    pub source_window_id: String,
    pub valid_until: Option<String>,
}
#[cfg(test)]
pub struct WorkingStateInput<'a> {
    pub item_kind: &'a str,
    pub semantic_key: &'a str,
    pub value: &'a Value,
    pub priority: i64,
    pub source_window_id: &'a str,
    pub valid_until: Option<&'a str>,
}
pub fn memory_enabled() -> bool {
    env::var("SAAA_MEMORY_ENABLED").as_deref() == Ok("1")
}
pub fn ensure_continuity_state(
    connection: &Connection,
    canonical_conversation_id: &str,
    now: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO continuity_state(
               id, canonical_conversation_id, context_policy_version, created_at, updated_at
             ) VALUES('primary', ?1, ?2, ?3, ?3)
             ON CONFLICT(id) DO UPDATE SET
               canonical_conversation_id=excluded.canonical_conversation_id,
               context_policy_version=excluded.context_policy_version,
               updated_at=excluded.updated_at
             WHERE continuity_state.canonical_conversation_id != excluded.canonical_conversation_id
                OR continuity_state.context_policy_version != excluded.context_policy_version",
        params![canonical_conversation_id, CONTEXT_POLICY_VERSION, now],
    )?;
    Ok(())
}
#[cfg(test)]
pub fn record_completed_turn(
    transaction: &Transaction<'_>,
    start_message_id: &str,
    end_message_id: &str,
    now: &str,
) -> Result<SourceWindow, String> {
    let start = load_source_message(transaction, start_message_id)?;
    let end = load_source_message(transaction, end_message_id)?;
    if start.conversation_id != end.conversation_id
        || start.task_mode != "conversation"
        || end.task_mode != "conversation"
        || !matches!(start.role.as_str(), "user" | "transcript")
        || end.role != "assistant"
        || start.ordinal >= end.ordinal
    {
        return Err("Memory source window must be one completed normal conversation turn".into());
    }
    let intervening_messages: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM conversation_messages
             WHERE conversation_id=?1 AND rowid>?2 AND rowid<?3
               AND role IN ('user','assistant','transcript')",
            params![start.conversation_id, start.ordinal, end.ordinal],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if intervening_messages != 0 {
        return Err("Memory source window cannot span multiple conversation turns".into());
    }
    let source_digest = digest_source_messages(&start, &end);
    let opaque_digest =
        sha256_hex(format!("{start_message_id}:{end_message_id}:{source_digest}").as_bytes());
    let window = SourceWindow {
        id: format!("memory_source_{}", &opaque_digest[..32]),
        source_ref: format!("saaa://memory-source/{}", &opaque_digest[..40]),
        start_message_id: start_message_id.to_string(),
        end_message_id: end_message_id.to_string(),
        source_digest,
    };
    transaction
        .execute(
            "INSERT INTO memory_source_windows(
               id, source_ref, start_message_id, end_message_id, source_digest,
               availability, created_at, updated_at
             ) VALUES(?1,?2,?3,?4,?5,'available',?6,?6)
             ON CONFLICT(source_ref) DO NOTHING",
            params![
                window.id,
                window.source_ref,
                window.start_message_id,
                window.end_message_id,
                window.source_digest,
                now
            ],
        )
        .map_err(database_error)?;
    let persisted = load_source_window_by_ref(transaction, &window.source_ref)?;
    if persisted != window {
        return Err("Memory source window idempotency conflict".into());
    }
    Ok(window)
}
pub fn cancel_unhandled_jobs(connection: &Connection, now: &str) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE memory_reflection_jobs
             SET status='cancelled', lease_until=NULL, next_attempt_at=NULL,
                 result_code='worker-unavailable', updated_at=?1
             WHERE status IN ('queued','running')",
        params![now],
    )
}
pub fn record_projection_event(
    connection: &Connection,
    health_state: &str,
    projected_bytes: usize,
    input_budget_bytes: usize,
    output_reserve_bytes: usize,
    repair_count: usize,
    now: &str,
) -> Result<(), String> {
    if !matches!(health_state, "green" | "yellow" | "red") {
        return Err("Invalid context health state".into());
    }
    let digest = sha256_hex(
        format!(
            "projection:{health_state}:{projected_bytes}:{input_budget_bytes}:{output_reserve_bytes}:{repair_count}:{now}"
        )
        .as_bytes(),
    );
    connection
        .execute(
            "INSERT OR IGNORE INTO context_projection_events(
               id,health_state,projected_bytes,input_budget_bytes,
               output_reserve_bytes,repair_count,created_at
             ) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                format!("context_projection_{}", &digest[..32]),
                health_state,
                projected_bytes,
                input_budget_bytes,
                output_reserve_bytes,
                repair_count,
                now
            ],
        )
        .map_err(database_error)?;
    trim_projection_events(connection)?;
    Ok(())
}
#[cfg(test)]
pub fn insert_profile_candidate(
    connection: &Connection,
    item_kind: &str,
    semantic_key: &str,
    value: &Value,
    priority: i64,
    source_window_id: &str,
    now: &str,
) -> Result<String, String> {
    validate_item(item_kind, semantic_key, value, priority, PROFILE_KINDS)?;
    require_available_source(connection, source_window_id)?;
    let encoded = serde_json::to_string(value).map_err(|error| error.to_string())?;
    let digest =
        sha256_hex(format!("profile:{semantic_key}:{source_window_id}:{encoded}").as_bytes());
    let id = format!("profile_{}", &digest[..32]);
    connection
        .execute(
            "INSERT INTO user_profile_items(
               id,item_kind,semantic_key,value_json,status,priority,source_window_id,created_at,updated_at
             ) VALUES(?1,?2,?3,?4,'candidate',?5,?6,?7,?7)
             ON CONFLICT(id) DO NOTHING",
            params![id, item_kind, semantic_key, encoded, priority, source_window_id, now],
        )
        .map_err(database_error)?;
    Ok(id)
}
#[cfg(test)]
pub fn confirm_profile_candidate(
    connection: &mut Connection,
    item_id: &str,
    now: &str,
) -> Result<(), String> {
    let transaction = connection.transaction().map_err(database_error)?;
    let semantic_key: String = transaction
        .query_row(
            "SELECT p.semantic_key FROM user_profile_items p
             JOIN memory_source_windows w ON w.id=p.source_window_id
             WHERE p.id=?1 AND p.status='candidate' AND w.availability='available'",
            params![item_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| "Profile candidate is unavailable".to_string())?;
    transaction
        .execute(
            "UPDATE user_profile_items SET status='superseded',updated_at=?1
             WHERE semantic_key=?2 AND status='active'",
            params![now, semantic_key],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "UPDATE user_profile_items SET status='active',updated_at=?1 WHERE id=?2",
            params![now, item_id],
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)
}
#[cfg(test)]
pub fn put_working_state(
    connection: &mut Connection,
    input: WorkingStateInput<'_>,
    now: &str,
) -> Result<String, String> {
    validate_item(
        input.item_kind,
        input.semantic_key,
        input.value,
        input.priority,
        WORKING_KINDS,
    )?;
    require_available_source(connection, input.source_window_id)?;
    validate_optional_timestamp(input.valid_until)?;
    let encoded = serde_json::to_string(input.value).map_err(|error| error.to_string())?;
    let digest = sha256_hex(
        format!(
            "working:{}:{}:{encoded}:{now}",
            input.semantic_key, input.source_window_id
        )
        .as_bytes(),
    );
    let id = format!("working_{}", &digest[..32]);
    let transaction = connection.transaction().map_err(database_error)?;
    transaction
        .execute(
            "UPDATE working_state_items SET status='superseded',updated_at=?1
             WHERE semantic_key=?2 AND status='active'",
            params![now, input.semantic_key],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "INSERT INTO working_state_items(
               id,item_kind,semantic_key,value_json,status,priority,source_window_id,
               valid_until,created_at,updated_at
             ) VALUES(?1,?2,?3,?4,'active',?5,?6,?7,?8,?8)",
            params![
                id,
                input.item_kind,
                input.semantic_key,
                encoded,
                input.priority,
                input.source_window_id,
                input.valid_until,
                now
            ],
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)?;
    Ok(id)
}
#[cfg(test)]
pub fn resolve_working_state(
    connection: &Connection,
    semantic_key: &str,
    now: &str,
) -> Result<usize, String> {
    connection
        .execute(
            "UPDATE working_state_items SET status='resolved',updated_at=?1
             WHERE semantic_key=?2 AND status='active'",
            params![now, semantic_key],
        )
        .map_err(database_error)
}
#[cfg(test)]
pub fn expire_working_state(connection: &Connection, now: &str) -> Result<usize, String> {
    connection
        .execute(
            "UPDATE working_state_items SET status='expired',updated_at=?1
             WHERE status='active' AND valid_until IS NOT NULL AND valid_until <= ?1",
            params![now],
        )
        .map_err(database_error)
}
#[cfg(test)]
pub fn activate_capsule_revision(
    connection: &mut Connection,
    source_max_created_at: &str,
    source_max_message_id: &str,
    source_digest: &str,
    items: &[CapsuleItemInput],
    now: &str,
) -> Result<i64, String> {
    if source_digest.len() != 64 || items.len() > 64 {
        return Err("Invalid capsule revision input".into());
    }
    let transaction = connection.transaction().map_err(database_error)?;
    if let Some(existing) = transaction
        .query_row(
            "SELECT revision FROM continuity_capsule_revisions
             WHERE source_digest=?1 AND status='active'",
            params![source_digest],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?
    {
        transaction.commit().map_err(database_error)?;
        return Ok(existing);
    }
    let revision: i64 = transaction
        .query_row(
            "SELECT COALESCE(MAX(revision),0)+1 FROM continuity_capsule_revisions",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    let revision_id = format!("capsule_{revision}_{}", &source_digest[..24]);
    let token_count = items
        .iter()
        .map(|item| serde_json::to_string(&item.value_json).map(|value| value.len()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?
        .into_iter()
        .sum::<usize>()
        .div_ceil(4);
    transaction
        .execute(
            "INSERT INTO continuity_capsule_revisions(
               id,revision,status,source_max_created_at,source_max_message_id,
               source_digest,token_count,created_at
             ) VALUES(?1,?2,'building',?3,?4,?5,?6,?7)",
            params![
                revision_id,
                revision,
                source_max_created_at,
                source_max_message_id,
                source_digest,
                token_count,
                now
            ],
        )
        .map_err(database_error)?;
    for (index, item) in items.iter().enumerate() {
        validate_item(
            &item.item_kind,
            &item.semantic_key,
            &item.value_json,
            item.priority,
            CAPSULE_KINDS,
        )?;
        validate_optional_timestamp(item.valid_until.as_deref())?;
        require_available_source(&transaction, &item.source_window_id)?;
        let encoded = serde_json::to_string(&item.value_json).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO continuity_capsule_items(
                   id,revision_id,item_kind,semantic_key,value_json,status,priority,
                   source_window_id,valid_until,created_at,updated_at
                 ) VALUES(?1,?2,?3,?4,?5,'active',?6,?7,?8,?9,?9)",
                params![
                    format!("{revision_id}_{index}"),
                    revision_id,
                    item.item_kind,
                    item.semantic_key,
                    encoded,
                    item.priority,
                    item.source_window_id,
                    item.valid_until,
                    now
                ],
            )
            .map_err(database_error)?;
    }
    transaction
        .execute(
            "UPDATE continuity_capsule_revisions
             SET status='superseded' WHERE status='active'",
            [],
        )
        .map_err(database_error)?;
    let activated = transaction
        .execute(
            "UPDATE continuity_capsule_revisions
             SET status='active',activated_at=?1 WHERE id=?2 AND status='building'",
            params![now, revision_id],
        )
        .map_err(database_error)?;
    if activated != 1 {
        return Err("Capsule revision could not be activated".into());
    }
    let state_changed = transaction
        .execute(
            "UPDATE continuity_state
             SET capsule_active_revision=?1,capsule_checkpoint_created_at=?2,
                 capsule_checkpoint_message_id=?3,updated_at=?4
             WHERE id='primary'",
            params![revision, source_max_created_at, source_max_message_id, now],
        )
        .map_err(database_error)?;
    if state_changed != 1 {
        return Err("Continuity state is unavailable for capsule activation".into());
    }
    transaction.commit().map_err(database_error)?;
    Ok(revision)
}
pub fn load_projection_items(
    connection: &Connection,
    now: &str,
) -> Result<Vec<ProjectionItem>, String> {
    let mut items = Vec::new();
    load_items(
        connection,
        "SELECT 'user_core',p.item_kind,p.semantic_key,p.value_json,w.source_ref,p.priority,NULL
         FROM user_profile_items p
         JOIN memory_source_windows w ON w.id=p.source_window_id
         WHERE p.status='active' AND w.availability='available'
         ORDER BY p.priority DESC,p.updated_at DESC LIMIT 32",
        params![],
        &mut items,
    )?;
    load_items(
        connection,
        "SELECT 'working_state',p.item_kind,p.semantic_key,p.value_json,w.source_ref,p.priority,p.valid_until
         FROM working_state_items p
         JOIN memory_source_windows w ON w.id=p.source_window_id
         WHERE p.status='active' AND w.availability='available'
           AND (p.valid_until IS NULL OR p.valid_until > ?1)
         ORDER BY p.priority DESC,p.updated_at DESC LIMIT 32",
        params![now],
        &mut items,
    )?;
    load_items(
        connection,
        "SELECT 'continuity_capsule',i.item_kind,i.semantic_key,i.value_json,w.source_ref,i.priority,i.valid_until
         FROM continuity_capsule_items i
         JOIN continuity_capsule_revisions r ON r.id=i.revision_id
         JOIN memory_source_windows w ON w.id=i.source_window_id
         JOIN continuity_state s ON s.capsule_active_revision=r.revision
         WHERE r.status='active' AND i.status='active' AND w.availability='available'
           AND (i.valid_until IS NULL OR i.valid_until > ?1)
         ORDER BY i.priority DESC,i.updated_at DESC LIMIT 64",
        params![now],
        &mut items,
    )?;
    let mut semantic_keys = HashSet::new();
    items.retain(|item| semantic_keys.insert(item.semantic_key.clone()));
    Ok(items)
}
