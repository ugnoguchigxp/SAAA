//! Closure of registry tables that route checks and audit rows actually touch.
//! The statements match the desktop initializers. Hosts that already create the
//! full application schema can skip these; an empty lab database must run them.
use rusqlite::{params, Connection, OptionalExtension};

use crate::database_error;

pub const MAX_ATTRIBUTES_JSON_BYTES: usize = 2_048;

const SETTINGS_AND_CREDENTIALS: &str = "
CREATE TABLE IF NOT EXISTS settings_documents (
  namespace TEXT NOT NULL,
  key TEXT NOT NULL,
  schema_version INTEGER NOT NULL,
  value_json TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(namespace, key)
);
CREATE TABLE IF NOT EXISTS credential_secrets (
  service TEXT NOT NULL,
  account TEXT NOT NULL,
  secret TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(service, account)
);
CREATE TABLE IF NOT EXISTS settings_revision (
  singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
  revision INTEGER NOT NULL CHECK(revision >= 0));
INSERT OR IGNORE INTO settings_revision(singleton, revision) VALUES(1, 0);
CREATE TRIGGER IF NOT EXISTS settings_revision_insert AFTER INSERT ON settings_documents
  BEGIN UPDATE settings_revision SET revision = revision + 1 WHERE singleton = 1; END;
CREATE TRIGGER IF NOT EXISTS settings_revision_update AFTER UPDATE ON settings_documents
  BEGIN UPDATE settings_revision SET revision = revision + 1 WHERE singleton = 1; END;
CREATE TRIGGER IF NOT EXISTS settings_revision_delete AFTER DELETE ON settings_documents
  BEGIN UPDATE settings_revision SET revision = revision + 1 WHERE singleton = 1; END;
";

pub fn initialize_settings_documents(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(SETTINGS_AND_CREDENTIALS)
        .map_err(database_error)
}

pub fn initialize_credential_secrets(connection: &Connection) -> Result<(), String> {
    initialize_settings_documents(connection)
}

/// Reads one named secret. Callers must not log the returned value.
pub fn read_named_secret(
    connection: &Connection,
    service: &str,
    account: &str,
) -> Result<Option<String>, String> {
    connection
        .query_row(
            "SELECT secret FROM credential_secrets WHERE service=?1 AND account=?2",
            params![service, account],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)
}

pub fn initialize_audit(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(&format!(
            r#"
        CREATE TABLE IF NOT EXISTS audit_events (
          sequence INTEGER PRIMARY KEY AUTOINCREMENT,
          id TEXT NOT NULL UNIQUE CHECK(length(id) BETWEEN 1 AND 160 AND id NOT GLOB '*[^A-Za-z0-9_-]*'),
          occurred_at TEXT NOT NULL CHECK(length(occurred_at) BETWEEN 1 AND 32),
          component TEXT NOT NULL CHECK(component IN (
            'app','frontend','microphone','voice-asr','conversation','provider','tts',
            'meeting','settings','voice-policy','situation'
          )),
          event_name TEXT NOT NULL CHECK(length(event_name) BETWEEN 1 AND 80 AND event_name NOT GLOB '*[^a-z0-9-]*'),
          phase TEXT NOT NULL CHECK(phase IN ('request','start','state','progress','decision','terminal','error')),
          outcome TEXT CHECK(outcome IS NULL OR outcome IN (
            'success','failure','cancelled','interrupted','degraded','blocked'
          )),
          correlation_id TEXT CHECK(correlation_id IS NULL OR (length(correlation_id) BETWEEN 1 AND 160 AND correlation_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          causation_id TEXT CHECK(causation_id IS NULL OR (length(causation_id) BETWEEN 1 AND 160 AND causation_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          conversation_id TEXT CHECK(conversation_id IS NULL OR (length(conversation_id) BETWEEN 1 AND 160 AND conversation_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          runtime_run_id TEXT CHECK(runtime_run_id IS NULL OR (length(runtime_run_id) BETWEEN 1 AND 160 AND runtime_run_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          session_id TEXT CHECK(session_id IS NULL OR (length(session_id) BETWEEN 1 AND 160 AND session_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          subject_id TEXT CHECK(subject_id IS NULL OR (length(subject_id) BETWEEN 1 AND 160 AND subject_id NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          failure_code TEXT CHECK(failure_code IS NULL OR (length(failure_code) BETWEEN 1 AND 160 AND failure_code NOT GLOB '*[^A-Za-z0-9_.:-]*')),
          attributes_json TEXT NOT NULL DEFAULT '{{}}'
            CHECK(length(attributes_json) <= {MAX_ATTRIBUTES_JSON_BYTES} AND json_valid(attributes_json) AND json_type(attributes_json) = 'object')
        );
        "#
        ))
        .map_err(database_error)
}
