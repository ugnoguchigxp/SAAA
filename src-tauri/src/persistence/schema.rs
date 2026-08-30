use rusqlite::{params, Connection};

use super::migrate::{
    migrate_direct_dynamic_lan_provider_to_discovery, migrate_legacy_settings_documents,
    migrate_pristine_provider_defaults_to_dynamic_lan, migrate_provider_reasoning_effort_default,
    migrate_v4_to_v5, migrate_v6_to_v7, migrate_v7_to_v8, migrate_v8_to_v9,
};
use super::provider_identity::migrate_dynamic_lan_provider_identity;
use super::runs::reconcile_interrupted_runs;
use super::settings::default_settings_documents;
use super::settings_migration::migrate_settings_v9_to_v10;
use crate::{meeting, memory, now_iso, voice, PRIMARY_CONVERSATION_ID, PRIMARY_CONVERSATION_TITLE};

pub(crate) fn initialize_database(connection: &Connection) -> rusqlite::Result<()> {
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
         CREATE TABLE IF NOT EXISTS provider_sessions (
           id TEXT PRIMARY KEY,
           provider_id TEXT NOT NULL,
           runtime_run_id TEXT CHECK(runtime_run_id IS NULL OR (length(runtime_run_id) BETWEEN 1 AND 160 AND runtime_run_id NOT GLOB '*[^A-Za-z0-9_-]*')),
           provider_kind TEXT CHECK(provider_kind IS NULL OR provider_kind IN ('openai-compatible', 'larm')),
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
         );
         CREATE TABLE IF NOT EXISTS meeting_sessions (
           id TEXT PRIMARY KEY,
           status TEXT NOT NULL CHECK(status IN ('active','paused','completed','saved','discarded','failed','interrupted')),
           microphone_enabled INTEGER NOT NULL CHECK(microphone_enabled IN (0,1)),
           system_audio_enabled INTEGER NOT NULL CHECK(system_audio_enabled IN (0,1)),
           stt_provider_id TEXT NOT NULL CHECK(stt_provider_id IN ('local-whisper','network-asr')),
           stt_model_label TEXT NOT NULL CHECK(length(stt_model_label) <= 256),
           translation_provider_id TEXT,
           persistence_mode TEXT NOT NULL CHECK(persistence_mode IN ('discard','explicit-save')),
           started_at TEXT NOT NULL, ended_at TEXT, saved_at TEXT, error_code TEXT
         );
         CREATE INDEX IF NOT EXISTS idx_meeting_sessions_started ON meeting_sessions(started_at DESC);
         CREATE TABLE IF NOT EXISTS meeting_transcript_entries (
           id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
           lane TEXT NOT NULL CHECK(lane IN ('microphone','system-audio')),
           sequence INTEGER NOT NULL CHECK(sequence >= 0),
           original_text TEXT NOT NULL CHECK(length(original_text) BETWEEN 1 AND 8000),
           original_language TEXT, translated_text TEXT CHECK(translated_text IS NULL OR length(translated_text) <= 8000), translated_language TEXT,
           started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0), ended_at_ms INTEGER NOT NULL CHECK(ended_at_ms >= started_at_ms), created_at TEXT NOT NULL,
           FOREIGN KEY(session_id) REFERENCES meeting_sessions(id) ON DELETE CASCADE, UNIQUE(session_id,lane,sequence)
         );
         CREATE INDEX IF NOT EXISTS idx_meeting_transcript_session_sequence ON meeting_transcript_entries(session_id,lane,sequence);",
    )?;

    let transaction = connection.unchecked_transaction()?;
    migrate_legacy_settings_documents(&transaction)?;
    migrate_v4_to_v5(&transaction)?;
    migrate_v6_to_v7(&transaction)?;
    migrate_v7_to_v8(&transaction)?;
    migrate_v8_to_v9(&transaction)?;
    memory::recall::migrate_v9_to_v10(&transaction)?;
    voice::profile::migrate_v10_to_v11(&transaction)?;
    memory::control_plane::migrate_v11_to_v12(&transaction)?;
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
    let memory_now = now_iso();
    memory::control_plane::ensure_continuity_state(
        &transaction,
        PRIMARY_CONVERSATION_ID,
        &memory_now,
    )?;
    memory::control_plane::recover_interrupted_jobs(&transaction, &memory_now)?;
    migrate_pristine_provider_defaults_to_dynamic_lan(&transaction)?;
    migrate_direct_dynamic_lan_provider_to_discovery(&transaction)?;
    migrate_dynamic_lan_provider_identity(&transaction)?;
    migrate_provider_reasoning_effort_default(&transaction)?;
    migrate_settings_v9_to_v10(&transaction)?;
    reconcile_interrupted_runs(&transaction)?;
    meeting::reconcile(&transaction)?;
    transaction.pragma_update(
        None,
        "user_version",
        memory::control_plane::MEMORY_SCHEMA_VERSION,
    )?;
    transaction.commit()
}
