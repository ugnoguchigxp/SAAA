use serde_json::json;
use super::*;
#[test]
    pub(super) fn version_eight_voice_and_meeting_schema_migrate_to_network_asr() {
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
            "DROP TABLE IF EXISTS meeting_transcript_entries;
             DROP TABLE IF EXISTS meeting_sessions;
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
        let meeting_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meeting_sessions')",
                [],
                |row| row.get(0),
            )
            .expect("meeting table check");
        let transcript_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meeting_transcript_entries')",
                [],
                |row| row.get(0),
            )
            .expect("transcript table check");
        assert_eq!(version, crate::persistence::schema::DATABASE_SCHEMA_VERSION);
        assert_eq!(voice.pointer("/allowedLanguages"), Some(&json!(["ja"])));
        assert_eq!(voice.pointer("/listeningEnabled"), Some(&json!(false)));
        assert!(voice.pointer("/sttProviderId").is_none());
        assert!(voice.pointer("/sttModel").is_none());
        assert!(!meeting_exists);
        assert!(!transcript_exists);
    }
#[test]
    pub(super) fn version_five_database_migrates_to_eight_without_losing_existing_data() {
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
        assert_eq!(version, crate::persistence::schema::DATABASE_SCHEMA_VERSION);
        assert!(!connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='meeting_transcript_entries')", [], |row| row.get::<_, bool>(0)).expect("meeting table dropped"));
        initialize_database(&connection).expect("migration idempotent");
    }
#[test]
    pub(super) fn version_six_calibration_is_inherited_with_new_input_defaults() {
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
    pub(super) fn version_six_settings_are_normalized_to_the_strict_v8_shape() {
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
        assert_eq!(documents.len(), 8);
        assert!(documents.iter().all(|document| {
            (document.namespace == "routing.roles"
                || document.schema_version == SETTINGS_SCHEMA_VERSION)
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
    pub(super) fn version_seven_settings_and_provider_sessions_migrate_to_v8() {
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
                "DROP TRIGGER IF EXISTS audit_provider_sessions_after_insert;
             DROP TRIGGER IF EXISTS audit_provider_sessions_after_update;
             DROP INDEX idx_provider_sessions_runtime_run;
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
        assert_eq!(version, crate::persistence::schema::DATABASE_SCHEMA_VERSION);
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
    pub(super) fn version_six_database_is_backed_up_before_v8() {
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
    pub(super) fn version_seven_database_is_backed_up_before_v8() {
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
