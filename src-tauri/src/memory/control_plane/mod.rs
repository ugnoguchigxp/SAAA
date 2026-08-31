//! SQLite-owned control plane for sessionless continuity and local memory work.
//! Raw conversation text remains owned exclusively by `conversation_messages`.
//!
//! Candidate mutation and worker APIs are staged behind `SAAA_MEMORY_ENABLED` until
//! the review UI and idle resource-signal adapters are connected.
#![allow(dead_code)]

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;
use std::{collections::HashSet, env};

pub const CONTEXT_POLICY_VERSION: i64 = 1;
// `user_version` is the application database version. Version 14 adds the
// conversation-scoped voice behavior policy and its bounded audit events.
pub const MEMORY_SCHEMA_VERSION: i64 = 14;
const MAX_ITEM_JSON_BYTES: usize = 4_000;
const MAX_SEMANTIC_KEY_BYTES: usize = 128;
const MAX_OBSERVABILITY_EVENTS: usize = 10_000;
const TURN_JOB_KINDS: [&str; 4] = [
    "capsule_refresh",
    "profile_candidate",
    "experience_reflection",
    "working_state_cleanup",
];

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapsuleItemInput {
    pub item_kind: String,
    pub semantic_key: String,
    pub value_json: Value,
    pub priority: i64,
    pub source_window_id: String,
    pub valid_until: Option<String>,
}

pub struct WorkingStateInput<'a> {
    pub item_kind: &'a str,
    pub semantic_key: &'a str,
    pub value: &'a Value,
    pub priority: i64,
    pub source_window_id: &'a str,
    pub valid_until: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryJob {
    pub id: String,
    pub job_kind: String,
    pub source_window_id: Option<String>,
    pub attempt_count: i64,
    pub lease_until: Option<String>,
}

pub fn memory_enabled() -> bool {
    env::var("SAAA_MEMORY_ENABLED").as_deref() == Ok("1")
}

mod items;
mod migrate;
use items::*;
pub use migrate::migrate_v11_to_v12;

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
    for job_kind in TURN_JOB_KINDS {
        enqueue_job(transaction, job_kind, Some(&window.id), now)?;
    }
    Ok(window)
}

pub fn recover_interrupted_jobs(connection: &Connection, now: &str) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE memory_reflection_jobs
             SET status='queued', lease_until=NULL, next_attempt_at=?1,
                 result_code='restart-recovered', updated_at=?1
             WHERE status='running'",
        params![now],
    )
}

pub fn record_decision_event(
    connection: &Connection,
    decision_kind: &str,
    result_code: &str,
    item_count: usize,
    now: &str,
) -> Result<(), String> {
    if decision_kind.is_empty()
        || decision_kind.len() > 64
        || result_code.is_empty()
        || result_code.len() > 64
    {
        return Err("Invalid bounded memory decision event".into());
    }
    let digest =
        sha256_hex(format!("decision:{decision_kind}:{result_code}:{item_count}:{now}").as_bytes());
    connection
        .execute(
            "INSERT OR IGNORE INTO memory_decision_events(
               id,decision_kind,result_code,item_count,created_at
             ) VALUES(?1,?2,?3,?4,?5)",
            params![
                format!("memory_decision_{}", &digest[..32]),
                decision_kind,
                result_code,
                item_count,
                now
            ],
        )
        .map_err(database_error)?;
    trim_observability_events(connection, "memory_decision_events")?;
    Ok(())
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
    trim_observability_events(connection, "context_projection_events")?;
    Ok(())
}

pub fn cancel_running_jobs(connection: &Connection, now: &str) -> Result<usize, String> {
    connection
        .execute(
            "UPDATE memory_reflection_jobs
             SET status='queued', lease_until=NULL, next_attempt_at=?1,
                 result_code='foreground-resumed', updated_at=?1
             WHERE status='running'",
            params![now],
        )
        .map_err(database_error)
}

pub fn claim_next_job(
    connection: &mut Connection,
    now: &str,
    lease_until: &str,
) -> Result<Option<MemoryJob>, String> {
    let transaction = connection.transaction().map_err(database_error)?;
    transaction
        .execute(
            "UPDATE memory_reflection_jobs
             SET status='queued',lease_until=NULL,next_attempt_at=?1,
                 result_code='lease-expired',updated_at=?1
             WHERE status='running' AND lease_until IS NOT NULL AND lease_until <= ?1",
            params![now],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "UPDATE memory_reflection_jobs
             SET status='failed',lease_until=NULL,result_code='attempts-exhausted',updated_at=?1
             WHERE status='queued' AND attempt_count >= 10",
            params![now],
        )
        .map_err(database_error)?;
    let candidate: Option<String> = transaction
        .query_row(
            "SELECT id FROM memory_reflection_jobs
             WHERE status='queued' AND attempt_count < 10
               AND (next_attempt_at IS NULL OR next_attempt_at <= ?1)
             ORDER BY created_at, id LIMIT 1",
            params![now],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    let Some(id) = candidate else {
        transaction.commit().map_err(database_error)?;
        return Ok(None);
    };
    let changed = transaction
        .execute(
            "UPDATE memory_reflection_jobs
             SET status='running', attempt_count=attempt_count+1, lease_until=?1,
                 result_code=NULL, updated_at=?2
             WHERE id=?3 AND status='queued'",
            params![lease_until, now, id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Memory job claim lost its atomic candidate".into());
    }
    let job = transaction
        .query_row(
            "SELECT id,job_kind,source_window_id,attempt_count,lease_until
             FROM memory_reflection_jobs WHERE id=?1",
            params![id],
            |row| {
                Ok(MemoryJob {
                    id: row.get(0)?,
                    job_kind: row.get(1)?,
                    source_window_id: row.get(2)?,
                    attempt_count: row.get(3)?,
                    lease_until: row.get(4)?,
                })
            },
        )
        .map_err(database_error)?;
    transaction.commit().map_err(database_error)?;
    Ok(Some(job))
}

pub fn finish_job(
    connection: &Connection,
    job_id: &str,
    status: &str,
    result_code: &str,
    now: &str,
) -> Result<(), String> {
    if !matches!(status, "completed" | "skipped" | "failed" | "cancelled")
        || result_code.is_empty()
        || result_code.len() > 64
    {
        return Err("Invalid memory job terminal result".into());
    }
    let changed = connection
        .execute(
            "UPDATE memory_reflection_jobs
             SET status=?1, result_code=?2, lease_until=NULL, updated_at=?3
             WHERE id=?4 AND status='running'",
            params![status, result_code, now, job_id],
        )
        .map_err(database_error)?;
    if changed != 1 {
        return Err("Memory job is not running".into());
    }
    Ok(())
}

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

pub fn expire_working_state(connection: &Connection, now: &str) -> Result<usize, String> {
    connection
        .execute(
            "UPDATE working_state_items SET status='expired',updated_at=?1
             WHERE status='active' AND valid_until IS NOT NULL AND valid_until <= ?1",
            params![now],
        )
        .map_err(database_error)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const T0: &str = "1787961600000";
    const T1: &str = "1787961660000";
    const T2: &str = "1787961720000";

    fn database() -> Connection {
        let connection = Connection::open_in_memory().expect("database opens");
        connection
            .execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE conversations (
                   id TEXT PRIMARY KEY,
                   title TEXT,
                   task_mode TEXT NOT NULL,
                   created_at TEXT NOT NULL,
                   updated_at TEXT NOT NULL
                 );
                 CREATE TABLE conversation_messages (
                   id TEXT PRIMARY KEY,
                   conversation_id TEXT NOT NULL,
                   role TEXT NOT NULL,
                   content TEXT NOT NULL,
                   created_at TEXT NOT NULL,
                   FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
                 );
                 INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
                 VALUES('primary','Primary','conversation','2026-08-29T00:00:00.000Z','2026-08-29T00:00:00.000Z');",
            )
            .expect("base schema creates");
        migrate_v11_to_v12(&connection).expect("memory schema migrates");
        ensure_continuity_state(&connection, "primary", T0).expect("continuity state initializes");
        connection
    }

    fn completed_source(connection: &mut Connection) -> SourceWindow {
        connection
            .execute_batch(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES
                   ('user-1','primary','user','Private project Alpha request','2026-08-29T00:00:10.000Z'),
                   ('assistant-1','primary','assistant','Private project Alpha response','2026-08-29T00:00:20.000Z');",
            )
            .expect("turn inserts");
        let transaction = connection.transaction().expect("transaction starts");
        let window =
            record_completed_turn(&transaction, "user-1", "assistant-1", T1).expect("turn records");
        transaction.commit().expect("turn commits");
        window
    }

    #[test]
    fn migration_is_idempotent_and_preserves_raw_conversation_rows() {
        let connection = database();
        connection
            .execute(
                "INSERT INTO conversation_messages(
                   id,conversation_id,role,content,created_at
                 ) VALUES('raw-kept','primary','user','Raw text remains unchanged',?1)",
                params![T0],
            )
            .expect("raw fixture inserts");
        let before: Vec<(String, String)> = connection
            .prepare("SELECT id,content FROM conversation_messages ORDER BY id")
            .expect("query prepares")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query runs")
            .collect::<Result<_, _>>()
            .expect("rows load");

        migrate_v11_to_v12(&connection).expect("second migration succeeds");

        let after: Vec<(String, String)> = connection
            .prepare("SELECT id,content FROM conversation_messages ORDER BY id")
            .expect("query prepares")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query runs")
            .collect::<Result<_, _>>()
            .expect("rows load");
        assert_eq!(before, after);
        let table_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN (
                   'continuity_state','memory_source_windows','continuity_capsule_revisions',
                   'user_profile_items','working_state_items','memory_reflection_jobs',
                   'memory_outbox','memory_decision_events','context_projection_events'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("tables count");
        assert_eq!(table_count, 9);
    }

    #[test]
    fn completed_turn_is_idempotent_and_enqueues_distinct_domain_jobs() {
        let mut connection = database();
        let first = completed_source(&mut connection);
        let transaction = connection.transaction().expect("transaction starts");
        let second = record_completed_turn(&transaction, "user-1", "assistant-1", T2)
            .expect("duplicate turn is accepted");
        transaction.commit().expect("transaction commits");

        assert_eq!(first, second);
        assert!(!first.source_ref.contains("Alpha"));
        let source_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM memory_source_windows", [], |row| {
                row.get(0)
            })
            .expect("source count");
        let job_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM memory_reflection_jobs", [], |row| {
                row.get(0)
            })
            .expect("job count");
        let distinct_kinds: i64 = connection
            .query_row(
                "SELECT COUNT(DISTINCT job_kind) FROM memory_reflection_jobs",
                [],
                |row| row.get(0),
            )
            .expect("job kinds count");
        assert_eq!((source_count, job_count, distinct_kinds), (1, 4, 4));

        connection
            .execute_batch(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                 VALUES
                   ('user-2','primary','user','Next request','1787961780000'),
                   ('assistant-2','primary','assistant','Next response','1787961840000');",
            )
            .expect("second turn inserts");
        let transaction = connection.transaction().expect("transaction starts");
        assert!(record_completed_turn(&transaction, "user-1", "assistant-2", T2).is_err());
    }

    #[test]
    fn only_confirmed_profile_items_project_and_source_deletion_tombstones_them() {
        let mut connection = database();
        let source = completed_source(&mut connection);
        let candidate = insert_profile_candidate(
            &connection,
            "communication",
            "response.style",
            &json!({"tone": "concise"}),
            10,
            &source.id,
            T1,
        )
        .expect("candidate inserts");
        let unavailable_candidate = insert_profile_candidate(
            &connection,
            "preference",
            "response.language",
            &json!({"language": "ja"}),
            5,
            &source.id,
            T1,
        )
        .expect("second candidate inserts");
        assert!(load_projection_items(&connection, T1)
            .expect("projection loads")
            .is_empty());

        confirm_profile_candidate(&mut connection, &candidate, T2).expect("candidate confirms");
        put_working_state(
            &mut connection,
            WorkingStateInput {
                item_kind: "constraint",
                semantic_key: "response.style",
                value: &json!({"tone": "temporary"}),
                priority: 100,
                source_window_id: &source.id,
                valid_until: None,
            },
            T2,
        )
        .expect("overlapping working state inserts");
        let projected = load_projection_items(&connection, T2).expect("projection loads");
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].memory_class, "user_core");

        connection
            .execute("DELETE FROM conversation_messages WHERE id='user-1'", [])
            .expect("source message deletes");
        let availability: (String, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT availability,start_message_id,end_message_id
                 FROM memory_source_windows WHERE id=?1",
                params![source.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("source tombstone loads");
        assert_eq!(availability, ("deleted".into(), None, None));
        assert!(load_projection_items(&connection, T2)
            .expect("projection loads")
            .is_empty());
        assert!(confirm_profile_candidate(&mut connection, &unavailable_candidate, T2).is_err());
    }

    #[test]
    fn working_state_honors_ttl_and_resolution() {
        let mut connection = database();
        let source = completed_source(&mut connection);
        put_working_state(
            &mut connection,
            WorkingStateInput {
                item_kind: "open_loop",
                semantic_key: "followup.pending",
                value: &json!({"summary": "follow up"}),
                priority: 20,
                source_window_id: &source.id,
                valid_until: Some(T2),
            },
            T1,
        )
        .expect("working item inserts");
        assert_eq!(
            load_projection_items(&connection, T1)
                .expect("projection loads")
                .len(),
            1
        );
        assert!(load_projection_items(&connection, T2)
            .expect("projection loads")
            .is_empty());
        assert_eq!(
            expire_working_state(&connection, T2).expect("item expires"),
            1
        );

        put_working_state(
            &mut connection,
            WorkingStateInput {
                item_kind: "commitment",
                semantic_key: "followup.pending",
                value: &json!({"summary": "new follow up"}),
                priority: 20,
                source_window_id: &source.id,
                valid_until: None,
            },
            T2,
        )
        .expect("replacement inserts");
        assert_eq!(
            resolve_working_state(&connection, "followup.pending", T2).expect("item resolves"),
            1
        );
        assert!(load_projection_items(&connection, T2)
            .expect("projection loads")
            .is_empty());
    }

    #[test]
    fn capsule_activation_is_atomic_and_keeps_one_active_revision() {
        let mut connection = database();
        let source = completed_source(&mut connection);
        let first = [CapsuleItemInput {
            item_kind: "active_referent".into(),
            semantic_key: "subject.current".into(),
            value_json: json!({"subject": "abstract client"}),
            priority: 10,
            source_window_id: source.id.clone(),
            valid_until: None,
        }];
        assert_eq!(
            activate_capsule_revision(
                &mut connection,
                "2026-08-29T00:00:20.000Z",
                "assistant-1",
                &"a".repeat(64),
                &first,
                T1,
            )
            .expect("first capsule activates"),
            1
        );
        let invalid = [CapsuleItemInput {
            item_kind: "invalid".into(),
            semantic_key: "subject.invalid".into(),
            value_json: json!(true),
            priority: 0,
            source_window_id: source.id.clone(),
            valid_until: None,
        }];
        assert!(activate_capsule_revision(
            &mut connection,
            "2026-08-29T00:00:20.000Z",
            "assistant-1",
            &"b".repeat(64),
            &invalid,
            T2,
        )
        .is_err());
        let statuses: (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*) FILTER (WHERE status='active'),
                        COUNT(*) FILTER (WHERE status='building')
                 FROM continuity_capsule_revisions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("statuses load");
        assert_eq!(statuses, (1, 0));
        assert_eq!(
            load_projection_items(&connection, T2)
                .expect("projection loads")
                .iter()
                .filter(|item| item.memory_class == "continuity_capsule")
                .count(),
            1
        );
    }

    #[test]
    fn job_claim_cancel_and_restart_recovery_are_durable() {
        let mut connection = database();
        completed_source(&mut connection);
        let claimed = claim_next_job(&mut connection, T1, T2)
            .expect("claim succeeds")
            .expect("job exists");
        assert_eq!(claimed.attempt_count, 1);
        finish_job(&connection, &claimed.id, "completed", "completed", T1).expect("job completes");
        claim_next_job(&mut connection, T1, T2)
            .expect("second claim succeeds")
            .expect("second job exists");
        assert_eq!(
            cancel_running_jobs(&connection, T1).expect("job cancels"),
            1
        );

        connection
            .execute(
                "UPDATE memory_reflection_jobs SET status='running'
                 WHERE id=(SELECT id FROM memory_reflection_jobs WHERE status='queued' LIMIT 1)",
                [],
            )
            .expect("running job simulates restart");
        assert_eq!(
            recover_interrupted_jobs(&connection, T2).expect("jobs recover"),
            1
        );
        let recovered: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM memory_reflection_jobs
                 WHERE status='queued' AND result_code='restart-recovered'",
                [],
                |row| row.get(0),
            )
            .expect("recovered jobs count");
        assert_eq!(recovered, 1);
    }
}
