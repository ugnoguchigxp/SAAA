use super::migrate::{
    ensure_provider_configuration_fingerprint, migrate_direct_dynamic_lan_provider_to_discovery,
    migrate_legacy_settings_documents, migrate_pristine_provider_defaults_to_dynamic_lan,
    migrate_provider_reasoning_effort_default, migrate_v26_to_v27, migrate_v4_to_v5,
    migrate_v6_to_v7, migrate_v7_to_v8, migrate_v8_to_v9,
};
use super::provider_identity::migrate_dynamic_lan_provider_identity;
use super::runs::reconcile_interrupted_runs;
use super::settings::default_settings_documents;
use super::settings_migration::migrate_settings_to_current;
use crate::{memory, now_iso, voice, PRIMARY_CONVERSATION_ID, PRIMARY_CONVERSATION_TITLE};
use rusqlite::{params, Connection};

/// Current schema. 26 added steward tables and generated-capability generation/inspection
/// tables. 27 dropped Meeting session tables. 28 adds the role-routing ledger; 29 adds its
/// local learning ledger. 30 adds the schedule ledger (CREATE IF NOT EXISTS only).
/// 31 adds steward execution progress, expanded task states, recipes, and source bindings.
pub(crate) const DATABASE_SCHEMA_VERSION: i64 = 31;

pub(crate) fn initialize_database(connection: &Connection) -> rusqlite::Result<()> {
    let previous_version: i64 =
        connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    connection.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         CREATE TABLE IF NOT EXISTS settings_documents (
           namespace TEXT NOT NULL,
           key TEXT NOT NULL,
           schema_version INTEGER NOT NULL,
           value_json TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           PRIMARY KEY(namespace, key)
         );
         CREATE TABLE IF NOT EXISTS conversations (
           id TEXT PRIMARY KEY,
           title TEXT,
           task_mode TEXT NOT NULL CHECK(task_mode IN ('conversation', 'coding')),
           created_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS conversation_messages (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL,
           role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system', 'transcript')),
           content TEXT NOT NULL,
           created_at TEXT NOT NULL,
           FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_conversation_messages_conversation_created
           ON conversation_messages(conversation_id, created_at);
         CREATE INDEX IF NOT EXISTS idx_conversation_messages_conversation_created_ms
           ON conversation_messages(conversation_id, CAST(created_at AS INTEGER) DESC, id DESC);
         CREATE TABLE IF NOT EXISTS provider_sessions (
           id TEXT PRIMARY KEY,
           provider_id TEXT NOT NULL,
           runtime_run_id TEXT CHECK(runtime_run_id IS NULL OR (length(runtime_run_id) BETWEEN 1 AND 160 AND runtime_run_id NOT GLOB '*[^A-Za-z0-9_-]*')),
           provider_kind TEXT CHECK(provider_kind IS NULL OR provider_kind IN ('openai-compatible', 'larm')),
           configuration_fingerprint TEXT NOT NULL DEFAULT '' CHECK(length(configuration_fingerprint) IN (0,64)),
           route_id TEXT CHECK(route_id IS NULL OR (length(route_id) BETWEEN 1 AND 80 AND route_id NOT GLOB '*[^A-Za-z0-9._-]*')),
           allocation_id TEXT CHECK(allocation_id IS NULL OR (length(allocation_id) BETWEEN 1 AND 160 AND allocation_id NOT GLOB '*[^A-Za-z0-9_-]*')),
           selected_runtime_id TEXT CHECK(selected_runtime_id IS NULL OR (length(selected_runtime_id) BETWEEN 1 AND 160 AND selected_runtime_id NOT GLOB '*[^A-Za-z0-9_-]*')),
           fallback_used INTEGER CHECK(fallback_used IS NULL OR fallback_used IN (0,1)),
           selection_reason TEXT CHECK(selection_reason IS NULL OR selection_reason IN ('primary', 'other')),
           request_id TEXT CHECK(request_id IS NULL OR (length(request_id) BETWEEN 1 AND 160 AND request_id NOT GLOB '*[^A-Za-z0-9_-]*')),
           output_started INTEGER CHECK(output_started IS NULL OR output_started IN (0,1)),
           failure_kind TEXT CHECK(failure_kind IS NULL OR failure_kind IN (
             'authentication','contract','protocol','request-too-large','internal','client-disconnected',
             'cancelled','partial-output','policy','capacity','unavailable','draining','upstream','network',
             'timeout','allocation-lost','allocation-outcome-unknown','not-ready'
           )),
           release_status TEXT NOT NULL DEFAULT 'not-applicable' CHECK(release_status IN ('not-applicable','not-started','pending','released','failed','deferred-to-ttl')),
           release_failure_kind TEXT CHECK(release_failure_kind IS NULL OR release_failure_kind IN ('network','timeout','authentication','protocol','upstream','internal')),
           status TEXT NOT NULL CHECK(status IN ('running', 'completed', 'failed', 'cancelled', 'interrupted')),
           failure_reason TEXT,
           started_at TEXT NOT NULL,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS runtime_runs (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL,
           route_kind TEXT NOT NULL CHECK(route_kind IN ('conversation.respond', 'coding.assist', 'voice.transcribe', 'voice.speak')),
           provider_id TEXT,
           status TEXT NOT NULL CHECK(status IN ('running', 'completed', 'failed', 'cancelled', 'interrupted')),
           error_message TEXT,
           failure_code TEXT CHECK(failure_code IS NULL OR failure_code IN (
             'user-cancelled','app-restarted','configuration-error','child-start-failed',
             'request-timeout','progress-timeout',
             'terminal-timeout','hard-timeout','child-exited','protocol-error',
             'policy-violation','provider-error','response-too-large','internal-error'
           )),
           supervisor_version TEXT CHECK(supervisor_version IS NULL OR length(supervisor_version) BETWEEN 1 AND 64),
           last_progress_at TEXT CHECK(last_progress_at IS NULL OR length(last_progress_at) BETWEEN 1 AND 32),
           input_message_id TEXT,
           started_at TEXT NOT NULL,
           completed_at TEXT,
           FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
         );
         CREATE INDEX IF NOT EXISTS idx_runtime_runs_conversation_started
           ON runtime_runs(conversation_id, started_at);
         CREATE TABLE IF NOT EXISTS codex_threads (
           conversation_id TEXT PRIMARY KEY,
           thread_id TEXT NOT NULL UNIQUE,
           model TEXT NOT NULL,
           workspace_path TEXT NOT NULL,
           updated_at TEXT NOT NULL,
           FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS situation_ledger (
           id TEXT PRIMARY KEY,
           observed_at TEXT NOT NULL,
           scene TEXT NOT NULL,
           confidence INTEGER NOT NULL CHECK(confidence BETWEEN 0 AND 100),
           user_attention TEXT NOT NULL CHECK(user_attention IN ('available', 'busy', 'unknown')),
           audio_environment TEXT NOT NULL CHECK(audio_environment IN ('silence', 'speech', 'multi-speaker', 'media', 'unknown')),
           proposed_attention TEXT NOT NULL CHECK(proposed_attention IN ('IGNORE', 'OBSERVE', 'SUGGEST', 'RESPOND')),
           actual_execution TEXT NOT NULL CHECK(actual_execution = 'NONE'),
           actual_presentation TEXT NOT NULL CHECK(actual_presentation = 'SILENT'),
           evidence_json TEXT NOT NULL,
           signal_health_json TEXT NOT NULL,
           decision_reasons_json TEXT NOT NULL,
           rule_version TEXT NOT NULL,
           policy_version TEXT NOT NULL,
           entry_kind TEXT NOT NULL CHECK(entry_kind IN ('transition', 'decision', 'heartbeat'))
         );
         CREATE INDEX IF NOT EXISTS idx_situation_ledger_observed
           ON situation_ledger(observed_at DESC);
         CREATE TABLE IF NOT EXISTS situation_feedback (
           ledger_id TEXT PRIMARY KEY,
           verdict TEXT NOT NULL CHECK(verdict IN ('accurate', 'inaccurate', 'unsure')),
           corrected_scene TEXT,
           created_at TEXT NOT NULL,
           FOREIGN KEY(ledger_id) REFERENCES situation_ledger(id) ON DELETE CASCADE
         );",
    )?;

    super::settings_migration::initialize_revision(connection)?;
    // D4 widens tool_selection_sources.kind to mcp_http. This rebuild touches a parent table, so
    // it must run with foreign keys disabled before the main schema transaction opens.
    crate::tool_selection::schema::migrate_sources_kind(connection)?;
    crate::steward::schema_execution::migrate_task_loop_states(connection)?;
    let transaction = connection.unchecked_transaction()?;
    migrate_legacy_settings_documents(&transaction)?;
    migrate_v4_to_v5(&transaction)?;
    migrate_v6_to_v7(&transaction)?;
    migrate_v7_to_v8(&transaction)?;
    migrate_v8_to_v9(&transaction)?;
    migrate_v26_to_v27(&transaction)?;
    memory::recall::migrate_v9_to_v10(&transaction)?;
    voice::profile::migrate_v10_to_v11(&transaction)?;
    memory::control_plane::migrate_v11_to_v12(&transaction)?;
    voice::profile::migrate_v14_to_v15(&transaction)?;
    ensure_provider_configuration_fingerprint(&transaction)?;
    transaction.execute("UPDATE settings_documents SET schema_version = 9, updated_at = ?1 WHERE schema_version < 9", params![now_iso()])?;

    for (namespace, key, schema_version, value) in default_settings_documents() {
        transaction.execute(
            "INSERT OR IGNORE INTO settings_documents(namespace, key, schema_version, value_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![namespace, key, schema_version, value.to_string(), now_iso()],
        )?;
    }
    transaction.execute(
        "INSERT OR IGNORE INTO conversations(id, title, task_mode, created_at, updated_at)
         VALUES (?1, ?2, 'conversation', ?3, ?3)",
        params![
            PRIMARY_CONVERSATION_ID,
            PRIMARY_CONVERSATION_TITLE,
            now_iso()
        ],
    )?;
    crate::voice_behavior::migrate(&transaction)?;
    crate::generative_ui::store::migrate(&transaction)?;
    crate::coding::repository::migrate(&transaction)?;
    crate::steward::schema::migrate(&transaction)?;
    crate::coding::recovery::reconcile(&transaction)
        .map_err(rusqlite::Error::InvalidParameterName)?;
    crate::runtime::context::schema::migrate(&transaction)?;
    crate::generated_capabilities::schema::migrate(&transaction)?;
    crate::generated_capabilities::generation::repository::interrupt_running(
        &transaction,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0),
    )
    .map_err(|error| rusqlite::Error::InvalidParameterName(error.encode()))?;
    crate::tool_selection::schema::migrate(&transaction)?;
    crate::role_routing::schema::migrate(&transaction)?;
    crate::role_routing::learning::schema::migrate(&transaction)?;
    crate::role_routing::recovery::reconcile_startup_in_transaction(
        &transaction,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0),
    )
    .map_err(rusqlite::Error::InvalidParameterName)?;
    crate::adaptive_improvement::migrate(&transaction)?;
    crate::schedule::migrate(&transaction)?;
    crate::role_routing::repository::capture_current_policy(
        &transaction,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0),
    )
    .map_err(rusqlite::Error::InvalidParameterName)?;
    // Version 25 binds learned corrections to the remote endpoint they were learned on. Existing
    // remote rules are recorded as unconfirmed rather than guessed onto the current endpoint.
    if previous_version < 25 {
        crate::tool_selection::mcp::schema::backfill_rule_source_bindings(&transaction)?;
    }
    memory::personal_state::schema::migrate(&transaction)?;
    let memory_now = now_iso();
    memory::control_plane::ensure_continuity_state(
        &transaction,
        PRIMARY_CONVERSATION_ID,
        &memory_now,
    )?;
    memory::control_plane::cancel_unhandled_jobs(&transaction, &memory_now)?;
    migrate_pristine_provider_defaults_to_dynamic_lan(&transaction)?;
    migrate_direct_dynamic_lan_provider_to_discovery(&transaction)?;
    migrate_dynamic_lan_provider_identity(&transaction)?;
    migrate_provider_reasoning_effort_default(&transaction)?;
    migrate_settings_to_current(&transaction)?;
    super::remove_legacy_provider::migrate(&transaction)?;
    reconcile_interrupted_runs(&transaction)?;
    super::audit::initialize_schema(&transaction)?;
    transaction.execute(
        "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,attributes_json)
         VALUES(?1,?2,'app','database-ready','terminal','success','{}')",
        params![crate::new_id("audit"), now_iso()],
    )?;
    transaction.pragma_update(None, "user_version", DATABASE_SCHEMA_VERSION)?;
    transaction.commit()
}
