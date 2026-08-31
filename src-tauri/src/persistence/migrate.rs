use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use super::settings::default_settings_documents;
use super::settings::SETTINGS_SCHEMA_VERSION;
use crate::backup::backup_connection_to;
use crate::{
    database_error, memory, now_iso, providers, situation, voice, DEFAULT_DYNAMIC_LAN_HOST,
    DYNAMIC_LAN_PROVIDER_ID,
};

pub(crate) fn ensure_provider_configuration_fingerprint(
    connection: &Connection,
) -> rusqlite::Result<()> {
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('provider_sessions')
         WHERE name='configuration_fingerprint')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        connection.execute_batch(
            "ALTER TABLE provider_sessions
             ADD COLUMN configuration_fingerprint TEXT NOT NULL DEFAULT ''
             CHECK(length(configuration_fingerprint) IN (0,64));",
        )?;
    }
    Ok(())
}

pub(crate) fn migrate_provider_reasoning_effort_default(
    connection: &Connection,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE settings_documents
         SET value_json = json_set(value_json, '$.reasoningEffort', ?1), updated_at = ?2
         WHERE namespace = 'providers.model'
           AND key = 'default'
           AND json_valid(value_json)
           AND json_type(value_json, '$.reasoningEffort') IS NULL",
        params![providers::DEFAULT_CONVERSATION_REASONING_EFFORT, now_iso()],
    )?;
    Ok(())
}

pub(crate) fn migrate_direct_dynamic_lan_provider_to_discovery(
    connection: &Connection,
) -> rusqlite::Result<()> {
    let current: Option<String> = connection
        .query_row(
            "SELECT value_json FROM settings_documents
             WHERE namespace='providers.model' AND key='default'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(current) = current else {
        return Ok(());
    };
    let mut value: Value = serde_json::from_str(&current).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let Some(items) = value.get_mut("providers").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let mut changed = false;
    for item in items {
        if item.get("id").and_then(Value::as_str) != Some(DYNAMIC_LAN_PROVIDER_ID)
            || item.get("kind").and_then(Value::as_str) != Some("openai-compatible")
            || item.get("location").and_then(Value::as_str) != Some("local")
        {
            continue;
        }
        let host = item
            .get("endpoint")
            .and_then(Value::as_str)
            .and_then(|endpoint| url::Url::parse(endpoint).ok())
            .and_then(|endpoint| endpoint.host_str().map(str::to_string))
            .filter(|host| providers::dynamic_lan::control_base_url(host).is_ok())
            .unwrap_or_else(|| DEFAULT_DYNAMIC_LAN_HOST.to_string());
        let enabled = item.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        *item = json!({
            "kind": "dynamic-lan",
            "id": DYNAMIC_LAN_PROVIDER_ID,
            "enabled": enabled,
            "label": "LAN LLM · Dynamic connection",
            "location": "local",
            "host": host
        });
        changed = true;
    }
    if changed {
        connection.execute(
            "UPDATE settings_documents SET value_json=?1, updated_at=?2
             WHERE namespace='providers.model' AND key='default'",
            params![value.to_string(), now_iso()],
        )?;
    }
    Ok(())
}

pub(crate) fn migrate_v4_to_v5(connection: &Connection) -> rusqlite::Result<()> {
    let has_impact: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('situation_feedback') WHERE name='impact')",
        [],
        |r| r.get(0),
    )?;
    if !has_impact {
        connection.execute_batch("ALTER TABLE situation_feedback ADD COLUMN impact TEXT NOT NULL DEFAULT 'none' CHECK(impact IN ('none','no-effect','harmful'));")?;
    }
    let has_reason: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('situation_feedback') WHERE name='reason_code')",
        [],
        |row| row.get(0),
    )?;
    if !has_reason {
        connection.execute_batch("ALTER TABLE situation_feedback ADD COLUMN reason_code TEXT CHECK(reason_code IS NULL OR reason_code IN ('wrong-scene','stale-signal','unstable-transition','unwanted-suggestion','missed-meeting-candidate','insufficient-evidence'));")?;
    }
    connection.execute_batch("CREATE TABLE IF NOT EXISTS situation_quality_windows (id TEXT PRIMARY KEY, started_at TEXT NOT NULL, ended_at TEXT NOT NULL, rule_version TEXT NOT NULL, counters_json TEXT NOT NULL CHECK(length(counters_json)<=4096), created_at TEXT NOT NULL); CREATE INDEX IF NOT EXISTS idx_situation_quality_windows_ended ON situation_quality_windows(CAST(ended_at AS INTEGER) DESC); CREATE TABLE IF NOT EXISTS situation_calibration_profiles (id TEXT PRIMARY KEY, rule_version TEXT NOT NULL UNIQUE, base_rule_version TEXT, status TEXT NOT NULL CHECK(status IN ('candidate','active','superseded','rejected','rolled-back')), parameters_json TEXT NOT NULL CHECK(length(parameters_json)<=2048), created_at TEXT NOT NULL, decided_at TEXT, decision_reason_code TEXT CHECK(decision_reason_code IS NULL OR decision_reason_code IN ('wrong-scene','stale-signal','unstable-transition','unwanted-suggestion','missed-meeting-candidate','insufficient-evidence')), FOREIGN KEY(base_rule_version) REFERENCES situation_calibration_profiles(rule_version)); CREATE UNIQUE INDEX IF NOT EXISTS idx_situation_calibration_one_active ON situation_calibration_profiles(status) WHERE status='active'; CREATE TABLE IF NOT EXISTS situation_calibration_runs (id TEXT PRIMARY KEY, profile_id TEXT NOT NULL, fixture_set_version TEXT NOT NULL, status TEXT NOT NULL CHECK(status IN ('completed','failed')), metrics_json TEXT CHECK(metrics_json IS NULL OR length(metrics_json)<=8192), error_code TEXT, started_at TEXT NOT NULL, completed_at TEXT NOT NULL, FOREIGN KEY(profile_id) REFERENCES situation_calibration_profiles(id) ON DELETE CASCADE); CREATE INDEX IF NOT EXISTS idx_situation_calibration_runs_completed ON situation_calibration_runs(completed_at DESC);")?;
    let parameters = serde_json::to_string(&situation::contracts::CalibrationParameters::default())
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    connection.execute("INSERT OR IGNORE INTO situation_calibration_profiles(id,rule_version,status,parameters_json,created_at,decided_at) VALUES('profile_mvp1_default','mvp1-rules-v1','active',?1,?2,?2)", params![parameters, now_iso()])?;
    Ok(())
}

pub(crate) fn migrate_v6_to_v7(connection: &Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 7 {
        return Ok(());
    }
    for (column, definition) in [
        (
            "failure_code",
            "TEXT CHECK(failure_code IS NULL OR failure_code IN ('user-cancelled','app-restarted','configuration-error','child-start-failed','request-timeout','progress-timeout','terminal-timeout','hard-timeout','child-exited','protocol-error','policy-violation','provider-error','response-too-large','internal-error'))",
        ),
        (
            "supervisor_version",
            "TEXT CHECK(supervisor_version IS NULL OR length(supervisor_version) BETWEEN 1 AND 64)",
        ),
        (
            "last_progress_at",
            "TEXT CHECK(last_progress_at IS NULL OR length(last_progress_at) BETWEEN 1 AND 32)",
        ),
    ] {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('runtime_runs') WHERE name=?1)",
            [column],
            |row| row.get(0),
        )?;
        if !exists {
            connection.execute_batch(&format!(
                "ALTER TABLE runtime_runs ADD COLUMN {column} {definition};"
            ))?;
        }
    }
    for (namespace, key, _, template) in default_settings_documents() {
        let template = settings_template_for_v7(namespace, template);
        let legacy: Option<String> = connection
            .query_row(
                "SELECT value_json FROM settings_documents
                 WHERE namespace=?1 AND key=?2 AND schema_version < 7",
                params![namespace, key],
                |row| row.get(0),
            )
            .optional()?;
        let Some(legacy) = legacy else {
            continue;
        };
        let value: Value = serde_json::from_str(&legacy).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let normalized = normalize_json_to_template(&value, &template);
        connection.execute(
            "UPDATE settings_documents
             SET schema_version=7, value_json=?1, updated_at=?2
             WHERE namespace=?3 AND key=?4",
            params![normalized.to_string(), now_iso(), namespace, key],
        )?;
    }
    Ok(())
}

pub(crate) fn settings_template_for_v7(namespace: &str, mut template: Value) -> Value {
    if namespace == "providers.model" {
        template["providers"] = json!([{
            "id": "local-openai-compatible",
            "enabled": false,
            "label": "Local OpenAI-compatible",
            "location": "local",
            "endpoint": "",
            "model": "",
            "credentialStatus": "not-configured"
        }]);
    }
    template
}

pub(crate) fn settings_template_for_legacy_v8_or_v9(namespace: &str, mut template: Value) -> Value {
    if namespace == "providers.model" {
        template["providers"] = json!([{
            "kind": "openai-compatible",
            "id": "local-openai-compatible",
            "enabled": false,
            "label": "Local OpenAI-compatible",
            "location": "local",
            "endpoint": "",
            "model": "",
            "credentialStatus": "not-configured"
        }]);
    }
    template
}

pub(crate) fn migrate_v7_to_v8(connection: &Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 8 {
        return Ok(());
    }
    for (column, definition) in [
        (
            "runtime_run_id",
            "TEXT CHECK(runtime_run_id IS NULL OR (length(runtime_run_id) BETWEEN 1 AND 160 AND runtime_run_id NOT GLOB '*[^A-Za-z0-9_-]*'))",
        ),
        (
            "provider_kind",
            "TEXT CHECK(provider_kind IS NULL OR provider_kind IN ('openai-compatible', 'larm'))",
        ),
        (
            "route_id",
            "TEXT CHECK(route_id IS NULL OR (length(route_id) BETWEEN 1 AND 80 AND route_id NOT GLOB '*[^A-Za-z0-9._-]*'))",
        ),
        (
            "allocation_id",
            "TEXT CHECK(allocation_id IS NULL OR (length(allocation_id) BETWEEN 1 AND 160 AND allocation_id NOT GLOB '*[^A-Za-z0-9_-]*'))",
        ),
        (
            "selected_runtime_id",
            "TEXT CHECK(selected_runtime_id IS NULL OR (length(selected_runtime_id) BETWEEN 1 AND 160 AND selected_runtime_id NOT GLOB '*[^A-Za-z0-9_-]*'))",
        ),
        (
            "fallback_used",
            "INTEGER CHECK(fallback_used IS NULL OR fallback_used IN (0,1))",
        ),
        (
            "selection_reason",
            "TEXT CHECK(selection_reason IS NULL OR selection_reason IN ('primary', 'other'))",
        ),
        (
            "request_id",
            "TEXT CHECK(request_id IS NULL OR (length(request_id) BETWEEN 1 AND 160 AND request_id NOT GLOB '*[^A-Za-z0-9_-]*'))",
        ),
        (
            "output_started",
            "INTEGER CHECK(output_started IS NULL OR output_started IN (0,1))",
        ),
        (
            "failure_kind",
            "TEXT CHECK(failure_kind IS NULL OR failure_kind IN ('authentication','contract','protocol','request-too-large','internal','client-disconnected','cancelled','partial-output','policy','capacity','unavailable','draining','upstream','network','timeout','allocation-lost','allocation-outcome-unknown','not-ready'))",
        ),
        (
            "release_status",
            "TEXT NOT NULL DEFAULT 'not-applicable' CHECK(release_status IN ('not-applicable','not-started','pending','released','failed','deferred-to-ttl'))",
        ),
        (
            "release_failure_kind",
            "TEXT CHECK(release_failure_kind IS NULL OR release_failure_kind IN ('network','timeout','authentication','protocol','upstream','internal'))",
        ),
    ] {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('provider_sessions') WHERE name=?1)",
            [column],
            |row| row.get(0),
        )?;
        if !exists {
            connection.execute_batch(&format!(
                "ALTER TABLE provider_sessions ADD COLUMN {column} {definition};"
            ))?;
        }
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_provider_sessions_runtime_run
         ON provider_sessions(runtime_run_id);",
    )?;

    for (namespace, key, _, template) in default_settings_documents() {
        let template = settings_template_for_legacy_v8_or_v9(namespace, template);
        let legacy: Option<String> = connection
            .query_row(
                "SELECT value_json FROM settings_documents
                 WHERE namespace=?1 AND key=?2 AND schema_version < 8",
                params![namespace, key],
                |row| row.get(0),
            )
            .optional()?;
        let Some(legacy) = legacy else {
            continue;
        };
        let value: Value = serde_json::from_str(&legacy).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let normalized = normalize_json_to_template(&value, &template);
        connection.execute(
            "UPDATE settings_documents
             SET schema_version=8, value_json=?1, updated_at=?2
             WHERE namespace=?3 AND key=?4",
            params![normalized.to_string(), now_iso(), namespace, key],
        )?;
    }
    Ok(())
}

pub(crate) fn migrate_v8_to_v9(connection: &Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 9 {
        return Ok(());
    }

    let meeting_schema: String = connection.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='meeting_sessions'",
        [],
        |row| row.get(0),
    )?;
    if !meeting_schema.contains("network-asr") {
        connection.execute_batch(
            "CREATE TABLE meeting_sessions_v9 (
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
             INSERT INTO meeting_sessions_v9
               SELECT * FROM meeting_sessions;
             CREATE TABLE meeting_transcript_entries_v9 (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               lane TEXT NOT NULL CHECK(lane IN ('microphone','system-audio')),
               sequence INTEGER NOT NULL CHECK(sequence >= 0),
               original_text TEXT NOT NULL CHECK(length(original_text) BETWEEN 1 AND 8000),
               original_language TEXT,
               translated_text TEXT CHECK(translated_text IS NULL OR length(translated_text) <= 8000),
               translated_language TEXT,
               started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0),
               ended_at_ms INTEGER NOT NULL CHECK(ended_at_ms >= started_at_ms),
               created_at TEXT NOT NULL,
               FOREIGN KEY(session_id) REFERENCES meeting_sessions_v9(id) ON DELETE CASCADE,
               UNIQUE(session_id,lane,sequence)
             );
             INSERT INTO meeting_transcript_entries_v9
               SELECT * FROM meeting_transcript_entries;
             DROP TABLE meeting_transcript_entries;
             DROP TABLE meeting_sessions;
             ALTER TABLE meeting_sessions_v9 RENAME TO meeting_sessions;
             ALTER TABLE meeting_transcript_entries_v9 RENAME TO meeting_transcript_entries;
             CREATE INDEX idx_meeting_sessions_started ON meeting_sessions(started_at DESC);
             CREATE INDEX idx_meeting_transcript_session_sequence
               ON meeting_transcript_entries(session_id,lane,sequence);",
        )?;
    }

    for (namespace, key, _, template) in default_settings_documents() {
        let template = settings_template_for_legacy_v8_or_v9(namespace, template);
        let legacy: Option<String> = connection
            .query_row(
                "SELECT value_json FROM settings_documents
                 WHERE namespace=?1 AND key=?2 AND schema_version < 9",
                params![namespace, key],
                |row| row.get(0),
            )
            .optional()?;
        let Some(legacy) = legacy else {
            continue;
        };
        let value: Value = serde_json::from_str(&legacy).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        let mut normalized = normalize_json_to_template(&value, &template);
        if namespace == "voice.runtime" {
            normalized["sttProviderId"] = json!(voice::network_asr::PROVIDER_ID);
            normalized["sttModel"] = json!(voice::network_asr::MODEL_ID);
        }
        connection.execute(
            "UPDATE settings_documents
             SET schema_version=9, value_json=?1, updated_at=?2
             WHERE namespace=?3 AND key=?4",
            params![normalized.to_string(), now_iso(), namespace, key],
        )?;
    }
    Ok(())
}

pub(crate) fn normalize_json_to_template(value: &Value, template: &Value) -> Value {
    match (value, template) {
        (Value::Object(value), Value::Object(template)) => Value::Object(
            template
                .iter()
                .map(|(key, template_value)| {
                    let normalized = value
                        .get(key)
                        .map(|value| normalize_json_to_template(value, template_value))
                        .unwrap_or_else(|| template_value.clone());
                    (key.clone(), normalized)
                })
                .collect(),
        ),
        (Value::Array(value), Value::Array(template)) if template.len() == 1 => Value::Array(
            value
                .iter()
                .map(|value| normalize_json_to_template(value, &template[0]))
                .collect(),
        ),
        _ => value.clone(),
    }
}

pub(crate) fn backup_before_migration(
    connection: &Connection,
    database_path: &std::path::Path,
) -> Result<Option<PathBuf>, String> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(database_error)?;
    let has_data = fs::metadata(database_path)
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false);
    let settings_current = connection
        .query_row(
            "SELECT COALESCE(MIN(schema_version), 0) FROM settings_documents",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        >= SETTINGS_SCHEMA_VERSION;
    if !has_data || (version >= memory::control_plane::MEMORY_SCHEMA_VERSION && settings_current) {
        return Ok(None);
    }
    let directory = database_path
        .parent()
        .ok_or_else(|| "Database path has no parent directory".to_string())?
        .join("backups");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("Could not create the migration backup directory: {error}"))?;
    let path = directory.join(format!("pre-migration-{}.sqlite3", now_iso()));
    backup_connection_to(connection, &path)?;
    Ok(Some(path))
}

pub(crate) fn migrate_pristine_provider_defaults_to_dynamic_lan(
    connection: &Connection,
) -> rusqlite::Result<()> {
    let legacy_providers = json!({
        "providers": [{
            "kind": "openai-compatible",
            "id": "local-openai-compatible",
            "enabled": false,
            "label": "Local OpenAI-compatible",
            "location": "local",
            "endpoint": "",
            "model": "",
            "credentialStatus": "not-configured"
        }]
    });
    let current: Option<(String, String)> = connection
        .query_row(
            "SELECT providers.value_json, routing.value_json
             FROM settings_documents AS providers
             JOIN settings_documents AS routing
               ON routing.namespace='routing.tasks' AND routing.key='default'
             WHERE providers.namespace='providers.model' AND providers.key='default'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((providers_text, routing_text)) = current else {
        return Ok(());
    };
    let providers: Value = serde_json::from_str(&providers_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })?;
    let routing: Value = serde_json::from_str(&routing_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    if providers != legacy_providers
        || routing.pointer("/conversationRespond/primaryProviderId")
            != Some(&json!("local-openai-compatible"))
    {
        return Ok(());
    }

    let defaults = default_settings_documents();
    let dynamic_lan_providers = defaults
        .iter()
        .find(|(namespace, key, _, _)| *namespace == "providers.model" && *key == "default")
        .map(|(_, _, _, value)| value)
        .expect("providers default exists");
    let mut dynamic_lan_routing = routing;
    dynamic_lan_routing["conversationRespond"]["primaryProviderId"] =
        json!(DYNAMIC_LAN_PROVIDER_ID);
    let updated_at = now_iso();
    connection.execute(
        "UPDATE settings_documents SET value_json=?1, updated_at=?2
         WHERE namespace='providers.model' AND key='default'",
        params![dynamic_lan_providers.to_string(), updated_at],
    )?;
    connection.execute(
        "UPDATE settings_documents SET value_json=?1, updated_at=?2
         WHERE namespace='routing.tasks' AND key='default'",
        params![dynamic_lan_routing.to_string(), updated_at],
    )?;
    Ok(())
}

pub(crate) fn migrate_legacy_settings_documents(connection: &Connection) -> rusqlite::Result<()> {
    migrate_document(connection, "providers.model", "default", |legacy| {
        let provider = json!({
            "id": legacy.get("id").and_then(Value::as_str).unwrap_or("local-openai-compatible"),
            "enabled": legacy.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            "label": legacy.get("label").and_then(Value::as_str).unwrap_or("Local OpenAI-compatible"),
            "location": legacy.get("location").and_then(Value::as_str).unwrap_or("local"),
            "endpoint": legacy.get("endpoint").and_then(Value::as_str).unwrap_or(""),
            "model": legacy.get("model").and_then(Value::as_str).unwrap_or(""),
            "credentialStatus": legacy.get("credentialStatus").and_then(Value::as_str).unwrap_or("not-configured")
        });
        json!({ "providers": [provider] })
    })?;
    migrate_document(connection, "providers.agent", "codex-sdk", |legacy| {
        json!({
            "enabled": legacy.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            "provider": "codex-sdk",
            "model": legacy.get("model").and_then(Value::as_str).unwrap_or(""),
            "runtimeMode": "app-server",
            "health": legacy.get("health").and_then(Value::as_str).unwrap_or("unchecked"),
            "sandboxMode": "read-only",
            "approvalPolicy": "never",
            "networkEnabled": false,
            "webSearchEnabled": false,
            "workspacePolicy": "select-per-conversation"
        })
    })?;
    migrate_document(connection, "routing.tasks", "default", |legacy| {
        json!({
            "conversationRespond": legacy.get("conversationRespond").cloned().unwrap_or_else(|| json!({
                "primaryProviderId": "local-openai-compatible", "fallbackProviderIds": [], "timeoutMs": 30000
            })),
            "codingAssist": {
                "providerId": "codex-sdk",
                "timeoutMs": legacy.pointer("/codingAssist/timeoutMs").and_then(Value::as_u64).unwrap_or(120000),
                "readOnly": true,
                "networkEnabled": false,
                "webSearchEnabled": false
            }
        })
    })?;
    migrate_document(connection, "voice.runtime", "default", |legacy| {
        json!({
            "inputDeviceId": legacy.get("inputDeviceId").and_then(Value::as_str).unwrap_or("default"),
            "captureMode": "continuous",
            "allowedLanguages": [voice::language::DEFAULT_LANGUAGE_CODE],
            "sttProviderId": "network-asr",
            "sttModel": "qwen3-asr-1.7b",
            "ttsProviderId": "system-tts",
            "ttsVoice": legacy.get("ttsVoice").and_then(Value::as_str).unwrap_or("default"),
            "autoSpeak": legacy.get("autoSpeak").and_then(Value::as_bool).unwrap_or(true),
            "cloudFallbackEnabled": false
        })
    })?;
    migrate_document(connection, "security.runtime", "default", |legacy| {
        json!({
            "credentialStorage": "environment",
            "localOnlyWhenSelected": legacy.get("localOnlyWhenSelected").and_then(Value::as_bool).unwrap_or(true),
            "diagnosticsRedaction": true
        })
    })?;
    Ok(())
}

pub(crate) fn migrate_document(
    connection: &Connection,
    namespace: &str,
    key: &str,
    transform: impl FnOnce(Value) -> Value,
) -> rusqlite::Result<()> {
    let legacy: Option<(i64, String)> = connection
        .query_row(
            "SELECT schema_version, value_json FROM settings_documents WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((schema_version, value_text)) = legacy else {
        return Ok(());
    };
    if schema_version >= 3 {
        return Ok(());
    }
    let legacy_value = serde_json::from_str(&value_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(error))
    })?;
    connection.execute(
        "UPDATE settings_documents SET schema_version = 3, value_json = ?1, updated_at = ?2
         WHERE namespace = ?3 AND key = ?4",
        params![
            transform(legacy_value).to_string(),
            now_iso(),
            namespace,
            key
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::list_settings_documents;
    use crate::persistence::provider_identity::migrate_dynamic_lan_provider_identity;
    use crate::{
        initialize_database, memory, situation, DEFAULT_DYNAMIC_LAN_HOST, DYNAMIC_LAN_PROVIDER_ID,
    };
    use rusqlite::Connection;
    use serde_json::{json, Value};

    #[test]
    fn normalizes_regressed_dynamic_lan_provider_id_and_routes() {
        let connection = Connection::open_in_memory().expect("in-memory sqlite");
        initialize_database(&connection).expect("migration succeeds");
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1
                 WHERE namespace='providers.model' AND key='default'",
                [json!({
                    "harness": { "address": "http://10.0.0.42:9810" },
                    "providers": [{
                        "kind": "dynamic-lan",
                        "id": "dynamic-lan",
                        "enabled": true,
                        "label": "Dynamic LAN LLM",
                        "location": "local",
                        "host": "10.0.0.42"
                    }],
                    "reasoningEffort": "medium"
                })
                .to_string()],
            )
            .expect("regressed provider fixture writes");
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1
                 WHERE namespace='routing.tasks' AND key='default'",
                [json!({
                    "conversationRespond": {
                        "source": "provider",
                        "primaryProviderId": "dynamic-lan",
                        "fallbackProviderIds": [],
                        "timeoutMs": 30000
                    },
                    "voiceTranscribe": {
                        "source": "harness",
                        "providerId": null,
                        "timeoutMs": 120000
                    },
                    "voiceSpeak": {
                        "source": "harness",
                        "providerId": null,
                        "timeoutMs": 30000
                    },
                    "codingAssist": {
                        "providerId": "codex-sdk",
                        "timeoutMs": 120000,
                        "readOnly": true,
                        "networkEnabled": false,
                        "webSearchEnabled": false
                    }
                })
                .to_string()],
            )
            .expect("regressed route fixture writes");

        migrate_dynamic_lan_provider_identity(&connection).expect("identity migrates");

        let documents = list_settings_documents(&connection).expect("documents load");
        let providers = documents
            .iter()
            .find(|document| document.namespace == "providers.model")
            .expect("providers exist");
        let routing = documents
            .iter()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing exists");
        assert_eq!(
            providers.value_json.pointer("/providers/0/id"),
            Some(&json!(DYNAMIC_LAN_PROVIDER_ID))
        );
        assert_eq!(
            routing
                .value_json
                .pointer("/conversationRespond/primaryProviderId"),
            Some(&json!(DYNAMIC_LAN_PROVIDER_ID))
        );
    }

    #[test]
    fn migration_creates_default_documents() {
        let connection = Connection::open_in_memory().expect("in-memory sqlite");
        initialize_database(&connection).expect("migration succeeds");
        let documents = list_settings_documents(&connection).expect("documents load");
        assert_eq!(documents.len(), 7);
        assert!(documents
            .iter()
            .all(|document| document.schema_version == SETTINGS_SCHEMA_VERSION));
        let providers = documents
            .iter()
            .find(|document| document.namespace == "providers.model")
            .expect("provider defaults exist");
        assert_eq!(
            providers.value_json.pointer("/providers/0/id"),
            Some(&json!(DYNAMIC_LAN_PROVIDER_ID))
        );
        assert_eq!(
            providers.value_json.pointer("/providers/0/kind"),
            Some(&json!("dynamic-lan"))
        );
        assert_eq!(
            providers.value_json.pointer("/providers/0/host"),
            Some(&json!(DEFAULT_DYNAMIC_LAN_HOST))
        );
        assert!(providers
            .value_json
            .pointer("/providers/0/endpoint")
            .is_none());
        assert!(providers.value_json.pointer("/providers/0/model").is_none());
        assert_eq!(
            providers.value_json.pointer("/providers/0/enabled"),
            Some(&json!(true))
        );
        assert_eq!(
            providers.value_json["providers"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(
            providers.value_json.pointer("/reasoningEffort"),
            Some(&json!("medium"))
        );
        let routing = documents
            .iter()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing defaults exist");
        assert_eq!(
            routing
                .value_json
                .pointer("/conversationRespond/primaryProviderId"),
            Some(&Value::Null)
        );
        assert_eq!(
            routing.value_json.pointer("/conversationRespond/source"),
            Some(&json!("harness"))
        );
        assert_eq!(
            routing
                .value_json
                .pointer("/conversationRespond/fallbackProviderIds"),
            Some(&json!([]))
        );
        let (version, active_profile): (i64, String) = (
            connection
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .expect("version reads"),
            connection
                .query_row(
                    "SELECT id FROM situation_calibration_profiles WHERE status='active'",
                    [],
                    |row| row.get(0),
                )
                .expect("active profile reads"),
        );
        assert_eq!(version, memory::control_plane::MEMORY_SCHEMA_VERSION);
        assert_eq!(active_profile, "profile_mvp1_default");
        let recall_schema_objects: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
             WHERE name IN (
               'conversation_messages_fts',
               'conversation_recall_cursors',
               'conversation_recall_attempts',
               'conversation_recall_receipts',
               'conversation_messages_recall_insert',
               'conversation_messages_recall_update',
               'conversation_messages_recall_delete'
             )",
                [],
                |row| row.get(0),
            )
            .expect("recall schema reads");
        assert_eq!(recall_schema_objects, 7);
        let input_message_column: bool = connection
            .query_row(
                "SELECT EXISTS(
               SELECT 1 FROM pragma_table_info('runtime_runs') WHERE name='input_message_id'
             )",
                [],
                |row| row.get(0),
            )
            .expect("runtime input column reads");
        assert!(input_message_column);
    }

    #[test]
    fn existing_provider_settings_gain_the_medium_reasoning_default() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection
            .execute(
                "UPDATE settings_documents
             SET value_json = json_remove(value_json, '$.reasoningEffort')
             WHERE namespace = 'providers.model' AND key = 'default'",
                [],
            )
            .expect("reasoning setting removes");

        initialize_database(&connection).expect("reasoning default migrates");

        let reasoning_effort: String = connection
            .query_row(
                "SELECT json_extract(value_json, '$.reasoningEffort')
             FROM settings_documents
             WHERE namespace = 'providers.model' AND key = 'default'",
                [],
                |row| row.get(0),
            )
            .expect("reasoning setting reads");
        assert_eq!(reasoning_effort, "medium");
    }

    #[test]
    fn existing_provider_settings_remove_the_legacy_output_token_setting() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection
            .execute(
                "UPDATE settings_documents
                 SET value_json = json_set(value_json, '$.maxOutputTokens', 2048)
                 WHERE namespace = 'providers.model' AND key = 'default'",
                [],
            )
            .expect("legacy output token setting writes");

        initialize_database(&connection).expect("legacy output token setting migrates");

        let max_output_tokens_exists: bool = connection
            .query_row(
                "SELECT json_type(value_json, '$.maxOutputTokens') IS NOT NULL
                 FROM settings_documents
                 WHERE namespace = 'providers.model' AND key = 'default'",
                [],
                |row| row.get(0),
            )
            .expect("output token setting absence reads");
        assert!(!max_output_tokens_exists);
    }

    #[test]
    fn existing_voice_settings_copy_the_dynamic_lan_host_once() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection
            .execute_batch(
                "UPDATE settings_documents
                 SET value_json = json_set(value_json, '$.providers[0].host', '192.168.0.130')
                 WHERE namespace = 'providers.model' AND key = 'default';
                 UPDATE settings_documents
                 SET value_json = json_remove(value_json, '$.harness')
                 WHERE namespace = 'providers.model' AND key = 'default';",
            )
            .expect("migration fixture writes");

        initialize_database(&connection).expect("ASR host default migrates");

        connection
            .execute(
                "UPDATE settings_documents
                 SET value_json = json_set(value_json, '$.providers[0].host', '192.168.0.131')
                 WHERE namespace = 'providers.model' AND key = 'default'",
                [],
            )
            .expect("provider host changes independently");
        initialize_database(&connection).expect("independent ASR host remains stable");

        let harness_address: String = connection
            .query_row(
                "SELECT json_extract(value_json, '$.harness.address')
                 FROM settings_documents
                 WHERE namespace = 'providers.model' AND key = 'default'",
                [],
                |row| row.get(0),
            )
            .expect("Harness address reads");
        assert_eq!(harness_address, "http://192.168.0.130:9810");
    }

    #[test]
    fn existing_voice_settings_gain_japanese_language_registration() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection
            .execute(
                "UPDATE settings_documents
                 SET schema_version=11,
                     value_json=json_set(json_remove(value_json, '$.allowedLanguages'), '$.captureMode', 'push-to-talk')
                 WHERE namespace='voice.runtime' AND key='default'",
                [],
            )
            .expect("legacy voice settings write");

        initialize_database(&connection).expect("voice language setting migrates");

        let (schema_version, listening_enabled, allowed_languages): (i64, bool, String) =
            connection
                .query_row(
                    "SELECT schema_version,
                        json_extract(value_json, '$.listeningEnabled'),
                        json_extract(value_json, '$.allowedLanguages')
                 FROM settings_documents
                 WHERE namespace='voice.runtime' AND key='default'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .expect("migrated voice settings read");
        assert_eq!(schema_version, SETTINGS_SCHEMA_VERSION);
        assert!(!listening_enabled);
        assert_eq!(allowed_languages, r#"["ja"]"#);
    }

    #[test]
    fn existing_agent_settings_gain_the_default_agent_name() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection
            .execute(
                "UPDATE settings_documents
                 SET value_json = json_remove(value_json, '$.agentName')
                 WHERE namespace = 'providers.agent' AND key = 'codex-sdk'",
                [],
            )
            .expect("agent name removes");

        initialize_database(&connection).expect("agent name default migrates");

        let agent_name: String = connection
            .query_row(
                "SELECT json_extract(value_json, '$.agentName')
                 FROM settings_documents
                 WHERE namespace = 'providers.agent' AND key = 'codex-sdk'",
                [],
                |row| row.get(0),
            )
            .expect("agent name reads");
        assert_eq!(agent_name, crate::DEFAULT_AGENT_NAME);
    }

    #[test]
    fn existing_agent_settings_gain_an_empty_user_name() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection
            .execute(
                "UPDATE settings_documents
                 SET value_json = json_remove(value_json, '$.userName')
                 WHERE namespace = 'providers.agent' AND key = 'codex-sdk'",
                [],
            )
            .expect("user name removes");

        initialize_database(&connection).expect("user name default migrates");

        let user_name: String = connection
            .query_row(
                "SELECT json_extract(value_json, '$.userName')
                 FROM settings_documents
                 WHERE namespace = 'providers.agent' AND key = 'codex-sdk'",
                [],
                |row| row.get(0),
            )
            .expect("user name reads");
        assert_eq!(user_name, crate::DEFAULT_USER_NAME);
    }

    #[test]
    fn pristine_previous_provider_defaults_migrate_to_dynamic_lan() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        let legacy_providers = json!({
            "providers": [{
                "kind": "openai-compatible",
                "id": "local-openai-compatible",
                "enabled": false,
                "label": "Local OpenAI-compatible",
                "location": "local",
                "endpoint": "",
                "model": "",
                "credentialStatus": "not-configured"
            }]
        });
        let legacy_routing = json!({
            "conversationRespond": {
                "primaryProviderId": "local-openai-compatible",
                "fallbackProviderIds": [],
                "timeoutMs": 45000
            },
            "codingAssist": {
                "providerId": "codex-sdk",
                "timeoutMs": 120000,
                "readOnly": true,
                "networkEnabled": false,
                "webSearchEnabled": false
            }
        });
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1
             WHERE namespace='providers.model' AND key='default'",
                [legacy_providers.to_string()],
            )
            .expect("legacy provider defaults write");
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1
             WHERE namespace='routing.tasks' AND key='default'",
                [legacy_routing.to_string()],
            )
            .expect("legacy routing defaults write");

        initialize_database(&connection).expect("default upgrade succeeds");
        let documents = list_settings_documents(&connection).expect("settings load");
        let providers = documents
            .iter()
            .find(|document| document.namespace == "providers.model")
            .expect("providers exist");
        let routing = documents
            .iter()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing exists");
        assert_eq!(
            providers.value_json.pointer("/providers/0/id"),
            Some(&json!(DYNAMIC_LAN_PROVIDER_ID))
        );
        assert_eq!(
            providers.value_json.pointer("/providers/0/kind"),
            Some(&json!("dynamic-lan"))
        );
        assert_eq!(
            providers.value_json.pointer("/providers/0/host"),
            Some(&json!(DEFAULT_DYNAMIC_LAN_HOST))
        );
        assert_eq!(
            routing
                .value_json
                .pointer("/conversationRespond/primaryProviderId"),
            Some(&json!(DYNAMIC_LAN_PROVIDER_ID))
        );
        assert_eq!(
            routing.value_json.pointer("/conversationRespond/timeoutMs"),
            Some(&json!(45000))
        );
    }

    #[test]
    fn direct_dynamic_lan_endpoint_migrates_to_host_only_discovery() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        let direct = json!({
            "providers": [{
                "kind": "openai-compatible",
                "id": DYNAMIC_LAN_PROVIDER_ID,
                "enabled": true,
                "label": "LAN LLM · Ornith",
                "location": "local",
                "endpoint": "http://192.168.0.77:8083/v1",
                "model": "ornith15-35b",
                "credentialStatus": "not-configured"
            }],
            "reasoningEffort": "medium"
        });
        connection
            .execute(
                "UPDATE settings_documents SET value_json=?1
             WHERE namespace='providers.model' AND key='default'",
                [direct.to_string()],
            )
            .expect("direct provider writes");

        initialize_database(&connection).expect("discovery migration succeeds");
        let value: String = connection
            .query_row(
                "SELECT value_json FROM settings_documents
             WHERE namespace='providers.model' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("provider settings read");
        let value: Value = serde_json::from_str(&value).expect("provider settings parse");
        assert_eq!(
            value.pointer("/providers/0/kind"),
            Some(&json!("dynamic-lan"))
        );
        assert_eq!(
            value.pointer("/providers/0/host"),
            Some(&json!("192.168.0.77"))
        );
        assert!(value.pointer("/providers/0/endpoint").is_none());
        assert!(value.pointer("/providers/0/model").is_none());
    }

    #[test]
    fn version_eight_voice_and_meeting_schema_migrate_to_network_asr() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection
            .execute(
                "UPDATE settings_documents
             SET schema_version=8,
                 value_json=json_set(value_json,
                   '$.sttProviderId','local-whisper',
                   '$.sttModel','/tmp/legacy-model.bin')
             WHERE namespace='voice.runtime' AND key='default'",
                [],
            )
            .expect("v8 voice settings write");
        connection
        .execute_batch(
            "DROP TABLE meeting_transcript_entries;
             DROP TABLE meeting_sessions;
             CREATE TABLE meeting_sessions (
               id TEXT PRIMARY KEY,
               status TEXT NOT NULL CHECK(status IN ('active','paused','completed','saved','discarded','failed','interrupted')),
               microphone_enabled INTEGER NOT NULL CHECK(microphone_enabled IN (0,1)),
               system_audio_enabled INTEGER NOT NULL CHECK(system_audio_enabled IN (0,1)),
               stt_provider_id TEXT NOT NULL CHECK(stt_provider_id = 'local-whisper'),
               stt_model_label TEXT NOT NULL CHECK(length(stt_model_label) <= 256),
               translation_provider_id TEXT,
               persistence_mode TEXT NOT NULL CHECK(persistence_mode IN ('discard','explicit-save')),
               started_at TEXT NOT NULL, ended_at TEXT, saved_at TEXT, error_code TEXT
             );
             CREATE TABLE meeting_transcript_entries (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               lane TEXT NOT NULL CHECK(lane IN ('microphone','system-audio')),
               sequence INTEGER NOT NULL CHECK(sequence >= 0),
               original_text TEXT NOT NULL CHECK(length(original_text) BETWEEN 1 AND 8000),
               original_language TEXT,
               translated_text TEXT CHECK(translated_text IS NULL OR length(translated_text) <= 8000),
               translated_language TEXT,
               started_at_ms INTEGER NOT NULL CHECK(started_at_ms >= 0),
               ended_at_ms INTEGER NOT NULL CHECK(ended_at_ms >= started_at_ms),
               created_at TEXT NOT NULL,
               FOREIGN KEY(session_id) REFERENCES meeting_sessions(id) ON DELETE CASCADE,
               UNIQUE(session_id,lane,sequence)
             );
             INSERT INTO meeting_sessions(
               id,status,microphone_enabled,system_audio_enabled,stt_provider_id,
               stt_model_label,persistence_mode,started_at,ended_at
             ) VALUES(
               'legacy-meeting','completed',1,0,'local-whisper','legacy-model.bin',
               'discard','1','2'
             );
             INSERT INTO meeting_transcript_entries(
               id,session_id,lane,sequence,original_text,started_at_ms,ended_at_ms,created_at
             ) VALUES(
               'legacy-entry','legacy-meeting','microphone',0,'kept transcript',0,1000,'2'
             );",
        )
        .expect("v8 meeting schema writes");
        connection
            .pragma_update(None, "user_version", 8)
            .expect("v8 version writes");

        initialize_database(&connection).expect("v9 migration succeeds");

        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version reads");
        let voice: String = connection
            .query_row(
                "SELECT value_json FROM settings_documents
             WHERE namespace='voice.runtime' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("voice settings read");
        let voice: Value = serde_json::from_str(&voice).expect("voice settings decode");
        let meeting_schema: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='meeting_sessions'",
                [],
                |row| row.get(0),
            )
            .expect("meeting schema reads");
        let transcript: String = connection
            .query_row(
                "SELECT original_text FROM meeting_transcript_entries
             WHERE id='legacy-entry'",
                [],
                |row| row.get(0),
            )
            .expect("legacy transcript remains");
        assert_eq!(version, memory::control_plane::MEMORY_SCHEMA_VERSION);
        assert_eq!(voice.pointer("/allowedLanguages"), Some(&json!(["ja"])));
        assert_eq!(voice.pointer("/listeningEnabled"), Some(&json!(false)));
        assert!(voice.pointer("/sttProviderId").is_none());
        assert!(voice.pointer("/sttModel").is_none());
        assert!(meeting_schema.contains("network-asr"));
        assert_eq!(transcript, "kept transcript");
        connection
            .execute("DELETE FROM meeting_sessions WHERE id='legacy-meeting'", [])
            .expect("cascading delete succeeds");
        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM meeting_transcript_entries WHERE id='legacy-entry'",
                [],
                |row| row.get(0),
            )
            .expect("cascade count reads");
        assert_eq!(remaining, 0);
    }

    #[test]
    fn version_five_database_migrates_to_eight_without_losing_existing_data() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection.execute("INSERT INTO conversations(id,title,task_mode,created_at,updated_at) VALUES('kept','Keep','conversation','1','1')", []).expect("conversation persists");
        connection
            .execute("UPDATE settings_documents SET schema_version=5", [])
            .expect("v5 settings fixture");
        connection
            .pragma_update(None, "user_version", 5)
            .expect("v5 fixture");
        initialize_database(&connection).expect("v8 migration");
        let conversation: String = connection
            .query_row(
                "SELECT title FROM conversations WHERE id='kept'",
                [],
                |row| row.get(0),
            )
            .expect("conversation retained");
        assert_eq!(conversation, "Keep");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version reads");
        assert_eq!(version, memory::control_plane::MEMORY_SCHEMA_VERSION);
        assert!(connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meeting_transcript_entries')", [], |row| row.get::<_, bool>(0)).expect("meeting table exists"));
        initialize_database(&connection).expect("migration idempotent");
    }

    #[test]
    fn version_six_calibration_is_inherited_with_new_input_defaults() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        let legacy_json = r#"{"classificationMinConfidence":75,"lowConfidenceMax":40,"enterSampleCount":4,"exitSampleCount":6,"cooldownMs":12000}"#;
        connection
            .execute(
                "UPDATE situation_calibration_profiles
             SET parameters_json=?1
             WHERE id='profile_mvp1_default'",
                [legacy_json],
            )
            .expect("legacy profile writes");
        connection
            .pragma_update(None, "user_version", 6)
            .expect("v6 fixture");

        initialize_database(&connection).expect("v8 migration");
        let (rule_version, parameters_json): (String, String) = connection
            .query_row(
                "SELECT rule_version,parameters_json
             FROM situation_calibration_profiles
             WHERE id='profile_mvp1_default' AND status='active'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("migrated profile reads");
        let parameters: situation::contracts::CalibrationParameters =
            serde_json::from_str(&parameters_json).expect("parameters decode");
        assert_eq!(rule_version, "mvp1-rules-v1");
        assert_eq!(parameters_json, legacy_json);
        assert_eq!(parameters.classification_min_confidence, 75);
        assert_eq!(parameters.enter_sample_count, 4);
        assert_eq!(parameters.input_active_max_ms, 30_000);
        assert_eq!(parameters.input_recent_max_ms, 300_000);
        initialize_database(&connection).expect("legacy profile remains readable after reopen");
    }

    #[test]
    fn version_six_settings_are_normalized_to_the_strict_v8_shape() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        connection
            .execute(
                "UPDATE settings_documents
             SET schema_version=6,
                 value_json=json_set(value_json, '$.legacyField', 'ignored')",
                [],
            )
            .expect("legacy top-level fields write");
        connection
            .execute(
                "UPDATE settings_documents
                 SET value_json=json_remove(value_json, '$.providers[1]')
                 WHERE namespace='providers.model'",
                [],
            )
            .expect("future provider fixture removes");
        connection
            .execute(
                "UPDATE settings_documents
             SET value_json=json_set(value_json, '$.providers[0].legacyProviderField', 1)
             WHERE namespace='providers.model'",
                [],
            )
            .expect("legacy nested field writes");
        connection
            .pragma_update(None, "user_version", 6)
            .expect("v6 fixture");

        initialize_database(&connection).expect("v8 migration");
        let documents = list_settings_documents(&connection).expect("strict settings load");
        assert_eq!(documents.len(), 7);
        assert!(documents.iter().all(|document| {
            document.schema_version == SETTINGS_SCHEMA_VERSION
                && document.value_json.get("legacyField").is_none()
        }));
        let providers = documents
            .iter()
            .find(|document| document.namespace == "providers.model")
            .and_then(|document| document.value_json.pointer("/providers/0"))
            .expect("provider remains");
        assert!(providers.get("legacyProviderField").is_none());
        assert_eq!(providers.get("kind"), Some(&json!("dynamic-lan")));
    }

    #[test]
    fn version_seven_settings_and_provider_sessions_migrate_to_v8() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("initial schema");
        let v7_providers = json!({
            "providers": [{
                "id": "provider-a", "enabled": true, "label": "Provider A", "location": "local",
                "endpoint": "http://127.0.0.1:11434/v1", "model": "kept-model", "credentialStatus": "not-configured"
            }, {
                "id": "provider-b", "enabled": true, "label": "Provider B", "location": "local",
                "endpoint": "http://127.0.0.1:11435/v1", "model": "model-b", "credentialStatus": "not-configured"
            }, {
                "id": "provider-c", "enabled": true, "label": "Provider C", "location": "local",
                "endpoint": "http://127.0.0.1:11436/v1", "model": "model-c", "credentialStatus": "not-configured"
            }]
        });
        connection
            .execute(
                "UPDATE settings_documents
             SET schema_version=7, value_json=?1
             WHERE namespace='providers.model' AND key='default'",
                [v7_providers.to_string()],
            )
            .expect("v7 provider fixture writes");
        let v7_routing = json!({
            "conversationRespond": {
                "primaryProviderId": "provider-a",
                "fallbackProviderIds": ["provider-b", "provider-c"],
                "timeoutMs": 30_000
            },
            "codingAssist": {
                "providerId": "codex-sdk", "timeoutMs": 120_000, "readOnly": true,
                "networkEnabled": false, "webSearchEnabled": false
            }
        });
        connection
            .execute(
                "UPDATE settings_documents SET schema_version=7, value_json=?1
             WHERE namespace='routing.tasks' AND key='default'",
                [v7_routing.to_string()],
            )
            .expect("v7 routing fixture writes");
        connection
            .execute("UPDATE settings_documents SET schema_version=7", [])
            .expect("v7 settings fixture writes");
        connection
            .execute_batch(
                "DROP INDEX idx_provider_sessions_runtime_run;
             ALTER TABLE provider_sessions DROP COLUMN runtime_run_id;
             ALTER TABLE provider_sessions DROP COLUMN provider_kind;
             ALTER TABLE provider_sessions DROP COLUMN route_id;
             ALTER TABLE provider_sessions DROP COLUMN allocation_id;
             ALTER TABLE provider_sessions DROP COLUMN selected_runtime_id;
             ALTER TABLE provider_sessions DROP COLUMN fallback_used;
             ALTER TABLE provider_sessions DROP COLUMN selection_reason;
             ALTER TABLE provider_sessions DROP COLUMN request_id;
             ALTER TABLE provider_sessions DROP COLUMN output_started;
             ALTER TABLE provider_sessions DROP COLUMN failure_kind;
             ALTER TABLE provider_sessions DROP COLUMN release_status;
             ALTER TABLE provider_sessions DROP COLUMN release_failure_kind;",
            )
            .expect("v7 provider session shape restores");
        connection
            .execute(
                "INSERT INTO provider_sessions(
               id, provider_id, status, started_at, updated_at
             ) VALUES('legacy-session', 'legacy-provider', 'completed', '1', '1')",
                [],
            )
            .expect("v7 provider session row writes");
        connection
            .pragma_update(None, "user_version", 7)
            .expect("v7 fixture");

        initialize_database(&connection).expect("v8 migration succeeds");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version reads");
        let provider_value: String = connection
            .query_row(
                "SELECT value_json FROM settings_documents
             WHERE namespace='providers.model' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("provider settings read");
        let provider_value: Value =
            serde_json::from_str(&provider_value).expect("provider settings decode");
        assert_eq!(version, memory::control_plane::MEMORY_SCHEMA_VERSION);
        assert_eq!(
            provider_value.pointer("/providers/0/kind"),
            Some(&json!("openai-compatible"))
        );
        assert_eq!(
            provider_value.pointer("/providers/0/model"),
            Some(&json!("kept-model"))
        );
        let provider_ids = provider_value["providers"]
            .as_array()
            .expect("provider list remains an array")
            .iter()
            .map(|provider| {
                provider["id"]
                    .as_str()
                    .expect("provider id remains a string")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            provider_ids,
            ["provider-a", "provider-b", "provider-c", "system-tts"]
        );
        let routing_value: String = connection
            .query_row(
                "SELECT value_json FROM settings_documents
             WHERE namespace='routing.tasks' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("routing settings read");
        let routing_value: Value =
            serde_json::from_str(&routing_value).expect("routing settings decode");
        assert_eq!(
            routing_value.pointer("/conversationRespond/fallbackProviderIds"),
            Some(&json!(["provider-b", "provider-c"]))
        );
        for column in [
            "runtime_run_id",
            "provider_kind",
            "allocation_id",
            "selected_runtime_id",
            "output_started",
            "release_status",
        ] {
            let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('provider_sessions') WHERE name=?1)",
                [column],
                |row| row.get(0),
            )
            .expect("provider session column check succeeds");
            assert!(exists, "missing provider session column: {column}");
        }
        let (runtime_run_id, fallback_used, output_started, release_status): (
            Option<String>,
            Option<bool>,
            Option<bool>,
            String,
        ) = connection
            .query_row(
                "SELECT runtime_run_id, fallback_used, output_started, release_status
             FROM provider_sessions WHERE id='legacy-session'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("legacy provider session reads");
        assert!(runtime_run_id.is_none());
        assert!(fallback_used.is_none());
        assert!(output_started.is_none());
        assert_eq!(release_status, "not-applicable");
        assert!(connection
            .execute(
                "INSERT INTO provider_sessions(
               id, provider_id, runtime_run_id, provider_kind, status, started_at, updated_at
             ) VALUES('invalid-session', 'provider', 'invalid run id', 'larm', 'failed', '1', '1')",
                [],
            )
            .is_err());
        connection
            .execute(
                "INSERT INTO provider_sessions(
               id, provider_id, runtime_run_id, provider_kind, route_id, selection_reason,
               release_status, status, started_at, updated_at
             ) VALUES(
               'bounded-session', 'provider', 'run_1', 'larm', 'llm-default', 'other',
               'deferred-to-ttl', 'completed', '1', '1'
             )",
                [],
            )
            .expect("bounded v8 provider session row writes");
        initialize_database(&connection).expect("v8 migration is idempotent");
    }

    #[test]
    fn version_six_database_is_backed_up_before_v8() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("v6.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
             VALUES('backup-kept','Keep','coding','1','1')",
                [],
            )
            .expect("fixture inserts");
        connection
            .pragma_update(None, "user_version", 6)
            .expect("v6 fixture");
        drop(connection);

        let connection = Connection::open(&path).expect("database reopens");
        let backup = backup_before_migration(&connection, &path)
            .expect("backup succeeds")
            .expect("v6 backup is created");
        initialize_database(&connection).expect("v8 migration succeeds");
        let backup_connection = Connection::open(backup).expect("backup reopens");
        let title: String = backup_connection
            .query_row(
                "SELECT title FROM conversations WHERE id='backup-kept'",
                [],
                |row| row.get(0),
            )
            .expect("backup data remains");
        let backup_version: i64 = backup_connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("backup version reads");
        assert_eq!(title, "Keep");
        assert_eq!(backup_version, 6);
    }

    #[test]
    fn version_seven_database_is_backed_up_before_v8() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("v7.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "UPDATE settings_documents
             SET schema_version=7,
                 value_json=json_remove(value_json, '$.providers[0].kind')
             WHERE namespace='providers.model'",
                [],
            )
            .expect("v7 provider fixture writes");
        connection
            .execute("UPDATE settings_documents SET schema_version=7", [])
            .expect("v7 settings fixture writes");
        connection
            .pragma_update(None, "user_version", 7)
            .expect("v7 fixture");
        drop(connection);

        let connection = Connection::open(&path).expect("database reopens");
        let backup = backup_before_migration(&connection, &path)
            .expect("backup succeeds")
            .expect("v7 backup is created");
        initialize_database(&connection).expect("v8 migration succeeds");
        let backup_connection = Connection::open(backup).expect("backup reopens");
        let backup_version: i64 = backup_connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("backup version reads");
        let backup_provider_kind: Option<String> = backup_connection
            .query_row(
                "SELECT json_extract(value_json, '$.providers[0].kind')
             FROM settings_documents
             WHERE namespace='providers.model' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("backup provider settings read");
        assert_eq!(backup_version, 7);
        assert!(backup_provider_kind.is_none());
        drop(backup_connection);
        drop(connection);

        let reopened = Connection::open(&path).expect("migrated database reopens");
        initialize_database(&reopened).expect("reopened v8 database validates");
        let migrated_provider_kind: String = reopened
            .query_row(
                "SELECT json_extract(value_json, '$.providers[0].kind')
             FROM settings_documents
             WHERE namespace='providers.model' AND key='default'",
                [],
                |row| row.get(0),
            )
            .expect("migrated provider settings read");
        assert_eq!(migrated_provider_kind, "dynamic-lan");
    }

    #[test]
    fn version_eleven_database_is_backed_up_before_memory_v12() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("v11.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
             VALUES('v11-kept','Keep','coding','1','1')",
                [],
            )
            .expect("fixture inserts");
        connection
            .pragma_update(None, "user_version", 11)
            .expect("v11 fixture");
        drop(connection);

        let connection = Connection::open(&path).expect("database reopens");
        let backup = backup_before_migration(&connection, &path)
            .expect("backup succeeds")
            .expect("v11 backup is created");
        initialize_database(&connection).expect("v12 migration succeeds");

        let migrated_version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("migrated version reads");
        let backup_connection = Connection::open(backup).expect("backup reopens");
        let backup_version: i64 = backup_connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("backup version reads");
        let title: String = backup_connection
            .query_row(
                "SELECT title FROM conversations WHERE id='v11-kept'",
                [],
                |row| row.get(0),
            )
            .expect("backup data remains");
        assert_eq!(
            migrated_version,
            memory::control_plane::MEMORY_SCHEMA_VERSION
        );
        assert_eq!(backup_version, 11);
        assert_eq!(title, "Keep");
    }

    #[test]
    fn settings_v9_is_backed_up_before_v10_shape_migration() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings-v9.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "UPDATE settings_documents
                 SET schema_version=9,
                     value_json=CASE
                       WHEN namespace='providers.model' THEN json_remove(value_json, '$.maxOutputTokens')
                       WHEN namespace='voice.runtime' THEN json_remove(value_json, '$.sttHost')
                       ELSE value_json
                     END",
                [],
            )
            .expect("v9 settings fixture writes");
        drop(connection);

        let connection = Connection::open(&path).expect("database reopens");
        let backup = backup_before_migration(&connection, &path)
            .expect("backup succeeds")
            .expect("settings backup is created");
        initialize_database(&connection).expect("settings v10 migration succeeds");

        let backup_connection = Connection::open(backup).expect("backup reopens");
        let (backup_schema, backup_tokens): (i64, Option<i64>) = backup_connection
            .query_row(
                "SELECT schema_version, json_extract(value_json, '$.maxOutputTokens')
                 FROM settings_documents
                 WHERE namespace='providers.model' AND key='default'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("v9 backup remains readable");
        assert_eq!(backup_schema, 9);
        assert!(backup_tokens.is_none());

        let migrated = list_settings_documents(&connection).expect("v10 settings load");
        assert!(migrated
            .iter()
            .all(|document| document.schema_version == SETTINGS_SCHEMA_VERSION));
    }

    #[test]
    fn current_database_with_a_missing_settings_table_is_backed_up_before_repair() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("missing-settings.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute(
                "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
                 VALUES('repair-kept','Keep','coding','1','1')",
                [],
            )
            .expect("user data inserts");
        connection
            .execute("DROP TABLE settings_documents", [])
            .expect("settings corruption fixture writes");
        drop(connection);

        let connection = Connection::open(&path).expect("database reopens");
        let backup = backup_before_migration(&connection, &path)
            .expect("backup succeeds")
            .expect("repair backup is created");
        let backup_connection = Connection::open(backup).expect("backup reopens");
        let title: String = backup_connection
            .query_row(
                "SELECT title FROM conversations WHERE id='repair-kept'",
                [],
                |row| row.get(0),
            )
            .expect("user data remains in backup");
        assert_eq!(title, "Keep");
    }

    #[test]
    fn version_six_runtime_rows_gain_nullable_supervisor_columns() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection
            .execute_batch(
                "ALTER TABLE runtime_runs DROP COLUMN failure_code;
             ALTER TABLE runtime_runs DROP COLUMN supervisor_version;
             ALTER TABLE runtime_runs DROP COLUMN last_progress_at;",
            )
            .expect("v6 columns remove");
        connection
            .pragma_update(None, "user_version", 6)
            .expect("v6 fixture");
        initialize_database(&connection).expect("v8 migration succeeds");
        for column in ["failure_code", "supervisor_version", "last_progress_at"] {
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('runtime_runs') WHERE name=?1)",
                    [column],
                    |row| row.get(0),
                )
                .expect("column check succeeds");
            assert!(exists, "missing column: {column}");
        }
        connection
            .execute(
                "INSERT INTO conversations(id,task_mode,created_at,updated_at)
             VALUES('nullable','coding','1','1')",
                [],
            )
            .expect("conversation inserts");
        connection
            .execute(
                "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at)
             VALUES('nullable-run','nullable','coding.assist','running','1')",
                [],
            )
            .expect("nullable migrated columns accept old rows");
    }

    #[test]
    fn startup_reconciles_unfinished_meeting_without_persisted_transcript() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("schema initializes");
        connection.execute("INSERT INTO meeting_sessions(id,status,microphone_enabled,system_audio_enabled,stt_provider_id,stt_model_label,persistence_mode,started_at) VALUES('meeting_recover','active',1,0,'local-whisper','model.bin','discard','1')", []).expect("meeting fixture");
        initialize_database(&connection).expect("startup reconciliation");
        let status: String = connection
            .query_row(
                "SELECT status FROM meeting_sessions WHERE id='meeting_recover'",
                [],
                |row| row.get(0),
            )
            .expect("status reads");
        let transcript_count: i64 = connection.query_row("SELECT COUNT(*) FROM meeting_transcript_entries WHERE session_id='meeting_recover'", [], |row| row.get(0)).expect("transcript count");
        assert_eq!(status, "interrupted");
        assert_eq!(transcript_count, 0);
    }

    #[test]
    fn version_three_database_migrates_to_four_without_losing_mvp_zero_state() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("v3.sqlite3");
        let connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("initial schema creates");
        connection
            .execute(
                "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
             VALUES ('kept-conversation', 'Keep me', 'coding', '1', '1')",
                [],
            )
            .expect("conversation inserts");
        connection
        .execute(
            "INSERT INTO codex_threads(conversation_id, thread_id, model, workspace_path, updated_at)
             VALUES ('kept-conversation', 'kept-thread', 'kept-model', '/tmp/kept', '1')",
            [],
        )
        .expect("thread inserts");
        connection
            .execute(
                "DELETE FROM settings_documents WHERE namespace = 'situation.runtime'",
                [],
            )
            .expect("v4 document removes");
        connection
            .execute("UPDATE settings_documents SET schema_version = 3", [])
            .expect("settings downgrade fixture");
        connection
            .execute(
                "UPDATE settings_documents
                 SET value_json=json_remove(value_json, '$.providers[1]')
                 WHERE namespace='providers.model'",
                [],
            )
            .expect("future provider fixture removes");
        connection
            .pragma_update(None, "user_version", 3)
            .expect("fixture version sets");
        drop(connection);

        let reopened = Connection::open(&path).expect("database reopens");
        initialize_database(&reopened).expect("v4 migration succeeds");
        let version: i64 = reopened
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version reads");
        assert_eq!(version, memory::control_plane::MEMORY_SCHEMA_VERSION);
        let documents = list_settings_documents(&reopened).expect("settings load");
        assert_eq!(documents.len(), 7);
        assert!(documents
            .iter()
            .all(|document| document.schema_version == SETTINGS_SCHEMA_VERSION));
        let thread: String = reopened
            .query_row(
                "SELECT thread_id FROM codex_threads WHERE conversation_id = 'kept-conversation'",
                [],
                |row| row.get(0),
            )
            .expect("thread remains");
        assert_eq!(thread, "kept-thread");
    }
}
