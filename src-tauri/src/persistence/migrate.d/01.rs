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

    let meeting_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meeting_sessions')",
        [],
        |row| row.get(0),
    )?;
    if meeting_exists {
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
pub(crate) fn migrate_v26_to_v27(connection: &Connection) -> rusqlite::Result<()> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 27 {
        return Ok(());
    }
    connection.execute_batch(
        "DROP TRIGGER IF EXISTS audit_meeting_sessions_after_insert;
         DROP TRIGGER IF EXISTS audit_meeting_sessions_after_update;
         DROP TRIGGER IF EXISTS audit_meeting_transcript_entries_after_insert;
         DROP TABLE IF EXISTS meeting_transcript_entries;
         DROP TABLE IF EXISTS meeting_sessions;",
    )?;
    Ok(())
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
    if !has_data
        || (version >= crate::persistence::schema::DATABASE_SCHEMA_VERSION && settings_current)
    {
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
