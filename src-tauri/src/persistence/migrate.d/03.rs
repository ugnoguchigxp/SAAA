use super::*;
use crate::persistence::list_settings_documents;
use crate::persistence::provider_identity::migrate_dynamic_lan_provider_identity;
use crate::{
        initialize_database, situation, DEFAULT_DYNAMIC_LAN_HOST, DYNAMIC_LAN_PROVIDER_ID,
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
        assert_eq!(documents.len(), 8);
        assert!(documents
            .iter()
            .all(|document| document.namespace == "routing.roles"
                || document.schema_version == SETTINGS_SCHEMA_VERSION));
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
        assert_eq!(version, crate::persistence::schema::DATABASE_SCHEMA_VERSION);
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
