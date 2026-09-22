#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_eleven_fields_are_removed_deterministically() {
        let mut provider = json!({
            "providers": [{
                "kind": "openai-compatible", "id": "cloud", "enabled": true,
                "label": "Cloud", "location": "cloud", "endpoint": "https://example.com/v1",
                "model": "model", "credentialStatus": "configured"
            }],
            "reasoningEffort": "medium", "maxOutputTokens": 4096
        });
        migrate_provider_document(&mut provider, "default", true);
        assert!(provider.get("maxOutputTokens").is_none());
        assert!(provider.get("harness").is_some());
        assert_eq!(provider["providers"][0]["authentication"], "api-key");
        assert!(provider["providers"][0].get("credentialStatus").is_none());

        let voice = migrated_voice_document(
            &json!({
                "inputDeviceId": "mic", "captureMode": "push-to-talk", "ttsVoice": "Kyoko"
            }),
            true,
        );
        assert_eq!(voice["listeningEnabled"], false);
        assert_eq!(voice["allowedLanguages"], json!(["ja"]));
        assert!(voice.get("captureMode").is_none());
        assert!(voice.get("ttsVoice").is_none());

        let mut security = json!({
            "credentialStorage": "environment", "localOnlyWhenSelected": true,
            "diagnosticsRedaction": true
        });
        migrate_security_document(&mut security);
        assert!(security.get("credentialStorage").is_none());
    }

    #[test]
    fn schema_upgrade_requires_fresh_ambient_listening_consent() {
        let migrated = migrated_voice_document(
            &json!({
                "listeningEnabled": true,
                "inputDeviceId": "default",
                "outputDeviceId": "default",
                "vadSensitivity": "medium",
                "silenceTimeoutMs": 1500,
                "allowedLanguages": ["ja"],
                "autoSpeak": true
            }),
            true,
        );
        assert_eq!(migrated["listeningEnabled"], false);

        let current = migrated_voice_document(&migrated, false);
        assert_eq!(current["listeningEnabled"], false);
    }

    #[test]
    fn fixed_asr_language_is_migrated_to_automatic_detection() {
        let mut provider = json!({
            "harness": { "address": "" },
            "providers": [{
                "kind": "cloud-asr", "id": "asr", "enabled": true,
                "label": "ASR", "location": "cloud", "endpoint": "https://example.com/v1",
                "model": "model", "language": "ja", "authentication": "none"
            }],
            "reasoningEffort": "medium"
        });
        migrate_provider_document(&mut provider, "default", false);
        assert_eq!(provider["providers"][0]["language"], "auto");
    }

    #[test]
    fn current_provider_settings_are_not_rewritten_or_reenabled() {
        let connection = Connection::open_in_memory().expect("database opens");
        connection
            .execute_batch(
                "CREATE TABLE settings_documents (
                   namespace TEXT NOT NULL, key TEXT NOT NULL, schema_version INTEGER NOT NULL,
                   value_json TEXT NOT NULL, updated_at TEXT NOT NULL,
                   PRIMARY KEY(namespace, key)
                 );",
            )
            .expect("settings table creates");
        let value = json!({
            "harness": { "address": "http://provider.local:9810" },
            "providers": [{
                "kind": "dynamic-lan", "id": "dynamic-lan-primary", "enabled": false,
                "label": "Disabled LAN", "location": "local", "host": "provider.local"
            }],
            "reasoningEffort": "medium"
        });
        connection
            .execute(
                "INSERT INTO settings_documents(namespace,key,schema_version,value_json,updated_at)
                 VALUES('providers.model','default',?1,?2,'unchanged')",
                params![SETTINGS_SCHEMA_VERSION, value.to_string()],
            )
            .expect("fixture inserts");

        migrate_settings_to_current(&connection).expect("migration succeeds");

        let (stored, updated_at): (String, String) = connection
            .query_row(
                "SELECT value_json, updated_at FROM settings_documents
                 WHERE namespace='providers.model' AND key='default'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("fixture reads");
        assert_eq!(serde_json::from_str::<Value>(&stored).unwrap(), value);
        assert_eq!(updated_at, "unchanged");
    }

    #[test]
    fn schema_fourteen_moves_the_obsolete_same_host_direct_route_to_agent_connection() {
        let providers = json!({
            "harness": { "address": "http://192.168.0.130:9810" },
            "providers": [{
                "kind": "dynamic-lan", "id": DYNAMIC_LAN_PROVIDER_ID, "enabled": true,
                "label": "Agent Connection", "location": "local", "host": "192.168.0.130"
            }, {
                "kind": "openai-compatible", "id": "lan-qwen-direct", "enabled": true,
                "label": "Old direct route", "location": "local",
                "endpoint": "http://192.168.0.130:8080/v1", "model": "old",
                "authentication": "none"
            }]
        });
        let mut routing = json!({
            "conversationRespond": {
                "source": "provider", "primaryProviderId": "lan-qwen-direct",
                "fallbackProviderIds": [], "timeoutMs": 240_000
            }
        });

        migrate_obsolete_direct_lan_route(&providers, &mut routing);

        assert_eq!(routing["conversationRespond"]["source"], "harness");
        assert_eq!(
            routing["conversationRespond"]["primaryProviderId"],
            Value::Null
        );
        assert_eq!(
            routing["conversationRespond"]["fallbackProviderIds"],
            json!([])
        );
        assert_eq!(routing["conversationRespond"]["timeoutMs"], 240_000);
    }

    #[test]
    fn schema_fourteen_preserves_unrelated_direct_routes() {
        let providers = json!({
            "harness": { "address": "http://192.168.0.130:9810" },
            "providers": [{
                "kind": "dynamic-lan", "id": DYNAMIC_LAN_PROVIDER_ID, "enabled": true,
                "label": "Agent Connection", "location": "local", "host": "192.168.0.130"
            }, {
                "kind": "openai-compatible", "id": "other", "enabled": true,
                "label": "Other", "location": "local",
                "endpoint": "http://192.168.0.131:8080/v1", "model": "model",
                "authentication": "none"
            }]
        });
        let mut routing = json!({
            "conversationRespond": {
                "source": "provider", "primaryProviderId": "other",
                "fallbackProviderIds": [], "timeoutMs": 30_000
            }
        });
        let before = routing.clone();

        migrate_obsolete_direct_lan_route(&providers, &mut routing);

        assert_eq!(routing, before);
    }

    #[test]
    fn schema_thirteen_documents_are_persisted_with_the_agent_connection_route() {
        let connection = Connection::open_in_memory().expect("database opens");
        connection
            .execute_batch(
                "CREATE TABLE settings_documents (
                   namespace TEXT NOT NULL, key TEXT NOT NULL, schema_version INTEGER NOT NULL,
                   value_json TEXT NOT NULL, updated_at TEXT NOT NULL,
                   PRIMARY KEY(namespace, key)
                 );",
            )
            .expect("settings table creates");
        let providers = json!({
            "harness": { "address": "http://192.168.0.130:9810" },
            "providers": [{
                "kind": "dynamic-lan", "id": DYNAMIC_LAN_PROVIDER_ID, "enabled": true,
                "label": "Provider Harness LLM", "location": "local", "host": "192.168.0.130"
            }, {
                "kind": "openai-compatible", "id": "lan-qwen-direct", "enabled": true,
                "label": "Old direct route", "location": "local",
                "endpoint": "http://192.168.0.130:8080/v1", "model": "old",
                "authentication": "none"
            }],
            "reasoningEffort": "medium"
        });
        let routing = json!({
            "conversationRespond": {
                "source": "provider", "primaryProviderId": "lan-qwen-direct",
                "fallbackProviderIds": [], "timeoutMs": 240_000
            }
        });
        for (namespace, value) in [("providers.model", providers), ("routing.tasks", routing)] {
            connection
                .execute(
                    "INSERT INTO settings_documents(namespace,key,schema_version,value_json,updated_at)
                     VALUES(?1,'default',13,?2,'before')",
                    params![namespace, value.to_string()],
                )
                .unwrap();
        }

        migrate_settings_to_current(&connection).expect("migration succeeds");

        let (version, raw): (i64, String) = connection
            .query_row(
                "SELECT schema_version,value_json FROM settings_documents
                 WHERE namespace='routing.tasks' AND key='default'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let routing: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(routing["conversationRespond"]["source"], "harness");
        assert_eq!(
            routing["conversationRespond"]["primaryProviderId"],
            Value::Null
        );
    }

    #[test]
    fn schema_fourteen_does_not_reenable_providers_or_revoke_existing_voice_consent() {
        let connection = Connection::open_in_memory().expect("database opens");
        connection
            .execute_batch(
                "CREATE TABLE settings_documents (
                   namespace TEXT NOT NULL, key TEXT NOT NULL, schema_version INTEGER NOT NULL,
                   value_json TEXT NOT NULL, updated_at TEXT NOT NULL,
                   PRIMARY KEY(namespace, key)
                 );",
            )
            .expect("settings table creates");
        let providers = json!({
            "harness": { "address": "http://provider.local:9810" },
            "providers": [{
                "kind": "dynamic-lan", "id": DYNAMIC_LAN_PROVIDER_ID, "enabled": false,
                "label": "Keep disabled", "location": "local", "host": "provider.local"
            }],
            "reasoningEffort": "medium"
        });
        let voice = json!({
            "listeningEnabled": true, "inputDeviceId": "default", "outputDeviceId": "default",
            "vadSensitivity": "medium", "silenceTimeoutMs": 1500,
            "allowedLanguages": ["ja"], "autoSpeak": true
        });
        for (namespace, value) in [("providers.model", providers), ("voice.runtime", voice)] {
            connection
                .execute(
                    "INSERT INTO settings_documents(namespace,key,schema_version,value_json,updated_at)
                     VALUES(?1,'default',13,?2,'before')",
                    params![namespace, value.to_string()],
                )
                .unwrap();
        }

        migrate_settings_to_current(&connection).expect("migration succeeds");

        let read = |namespace: &str| -> Value {
            connection
                .query_row(
                    "SELECT value_json FROM settings_documents WHERE namespace=?1 AND key='default'",
                    [namespace],
                    |row| row.get::<_, String>(0),
                )
                .map(|raw| serde_json::from_str(&raw).unwrap())
                .unwrap()
        };
        assert_eq!(read("providers.model")["providers"][0]["enabled"], false);
        assert_eq!(
            read("providers.model")["providers"][0]["label"],
            "Keep disabled"
        );
        assert_eq!(read("voice.runtime")["listeningEnabled"], true);
    }
}
