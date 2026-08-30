use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use super::settings::SETTINGS_SCHEMA_VERSION;
use crate::{now_iso, DEFAULT_AGENT_NAME, DEFAULT_DYNAMIC_LAN_HOST, DEFAULT_USER_NAME};

struct StoredDocument {
    schema_version: i64,
    value: Value,
}

fn read_document(
    connection: &Connection,
    namespace: &str,
    key: &str,
) -> rusqlite::Result<Option<StoredDocument>> {
    let raw = connection
        .query_row(
            "SELECT schema_version, value_json FROM settings_documents
             WHERE namespace=?1 AND key=?2 AND json_valid(value_json)",
            params![namespace, key],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    Ok(raw.and_then(|(schema_version, raw)| {
        serde_json::from_str(&raw).ok().map(|value| StoredDocument {
            schema_version,
            value,
        })
    }))
}

fn write_document(
    connection: &Connection,
    namespace: &str,
    key: &str,
    value: &Value,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE settings_documents
         SET value_json=?1, schema_version=?2, updated_at=?3
         WHERE namespace=?4 AND key=?5",
        params![
            value.to_string(),
            SETTINGS_SCHEMA_VERSION,
            now_iso(),
            namespace,
            key
        ],
    )?;
    Ok(())
}

fn dynamic_lan_host(providers: &[Value]) -> &str {
    providers
        .iter()
        .find_map(|provider| {
            (provider.get("kind")?.as_str()? == "dynamic-lan")
                .then(|| provider.get("host")?.as_str())?
        })
        .unwrap_or(DEFAULT_DYNAMIC_LAN_HOST)
}

fn migrate_provider_document(value: &mut Value, system_voice: &str, legacy_shape: bool) {
    let Some(document) = value.as_object_mut() else {
        return;
    };
    document.remove("maxOutputTokens");
    let providers = document
        .entry("providers")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(providers) = providers.as_array_mut() else {
        return;
    };
    for provider in providers.iter_mut().filter_map(Value::as_object_mut) {
        if legacy_shape && provider.get("id").and_then(Value::as_str) == Some("system-tts") {
            *provider = json!({
                "kind": "system-tts",
                "id": "system-tts",
                "enabled": true,
                "label": "System Voice",
                "location": "local",
                "voice": system_voice
            })
            .as_object()
            .expect("system TTS fixture is an object")
            .clone();
            continue;
        }
        if legacy_shape && provider.get("kind").and_then(Value::as_str) == Some("dynamic-lan") {
            provider.insert("enabled".to_string(), Value::Bool(true));
            provider.insert(
                "label".to_string(),
                Value::String("Provider Harness LLM".to_string()),
            );
        }
        if provider.get("kind").and_then(Value::as_str) == Some("openai-compatible") {
            let authentication = match provider
                .remove("credentialStatus")
                .as_ref()
                .and_then(Value::as_str)
            {
                Some("configured") => "api-key",
                _ => "none",
            };
            provider
                .entry("authentication")
                .or_insert_with(|| Value::String(authentication.to_string()));
        }
        if provider.get("kind").and_then(Value::as_str) == Some("cloud-asr") {
            provider.insert("language".to_string(), Value::String("auto".to_string()));
        }
    }
    if legacy_shape
        && !providers
            .iter()
            .any(|provider| provider.get("id").and_then(Value::as_str) == Some("system-tts"))
    {
        providers.push(json!({
            "kind": "system-tts",
            "id": "system-tts",
            "enabled": true,
            "label": "System Voice",
            "location": "local",
            "voice": system_voice
        }));
    }
    let host = dynamic_lan_host(providers).to_string();
    document
        .entry("harness")
        .or_insert_with(|| json!({ "address": format!("http://{host}:9810") }));
    document
        .entry("reasoningEffort")
        .or_insert_with(|| Value::String("medium".to_string()));
}

fn migrate_routing_document(value: &mut Value) {
    let Some(document) = value.as_object_mut() else {
        return;
    };
    if let Some(conversation) = document
        .get_mut("conversationRespond")
        .and_then(Value::as_object_mut)
    {
        if !conversation.contains_key("source") {
            let harness_selected = conversation
                .get("primaryProviderId")
                .and_then(Value::as_str)
                == Some("dynamic-lan-primary");
            conversation.insert(
                "source".to_string(),
                Value::String(
                    if harness_selected {
                        "harness"
                    } else {
                        "provider"
                    }
                    .to_string(),
                ),
            );
            if harness_selected {
                conversation.insert("primaryProviderId".to_string(), Value::Null);
                conversation.insert("fallbackProviderIds".to_string(), json!([]));
            }
        }
    }
    document
        .entry("voiceTranscribe")
        .or_insert_with(|| json!({ "source": "harness", "providerId": null, "timeoutMs": 120000 }));
    document.entry("voiceSpeak").or_insert_with(
        || json!({ "source": "provider", "providerId": "system-tts", "timeoutMs": 30000 }),
    );
}

fn migrated_voice_document(value: &Value) -> Value {
    json!({
        "listeningEnabled": value.get("listeningEnabled").and_then(Value::as_bool).unwrap_or(true),
        "inputDeviceId": value.get("inputDeviceId").and_then(Value::as_str).unwrap_or("default"),
        "outputDeviceId": value.get("outputDeviceId").and_then(Value::as_str).unwrap_or("default"),
        "vadSensitivity": value.get("vadSensitivity").and_then(Value::as_str).unwrap_or("medium"),
        "silenceTimeoutMs": value.get("silenceTimeoutMs").and_then(Value::as_u64).unwrap_or(1500),
        "allowedLanguages": value.get("allowedLanguages").cloned().unwrap_or_else(|| json!([crate::voice::language::DEFAULT_LANGUAGE_CODE])),
        "autoSpeak": value.get("autoSpeak").and_then(Value::as_bool).unwrap_or(true)
    })
}

fn migrate_security_document(value: &mut Value) {
    let Some(document) = value.as_object_mut() else {
        return;
    };
    document.remove("credentialStorage");
    document
        .entry("localOnlyWhenSelected")
        .or_insert(Value::Bool(true));
    document
        .entry("diagnosticsRedaction")
        .or_insert(Value::Bool(true));
}

fn migrate_agent_document(value: &mut Value) {
    let Some(document) = value.as_object_mut() else {
        return;
    };
    document
        .entry("agentName")
        .or_insert_with(|| Value::String(DEFAULT_AGENT_NAME.to_string()));
    document
        .entry("userName")
        .or_insert_with(|| Value::String(DEFAULT_USER_NAME.to_string()));
}

pub(crate) fn migrate_settings_to_current(connection: &Connection) -> rusqlite::Result<()> {
    let old_voice = read_document(connection, "voice.runtime", "default")?;
    let system_voice = old_voice
        .as_ref()
        .and_then(|voice| voice.value.get("ttsVoice"))
        .and_then(Value::as_str)
        .unwrap_or("default");

    if let Some(mut providers) = read_document(connection, "providers.model", "default")? {
        let before = providers.value.clone();
        let legacy_shape = providers.schema_version < SETTINGS_SCHEMA_VERSION
            || providers.value.get("maxOutputTokens").is_some()
            || providers
                .value
                .get("providers")
                .and_then(Value::as_array)
                .is_some_and(|items| {
                    items
                        .iter()
                        .any(|provider| provider.get("credentialStatus").is_some())
                });
        migrate_provider_document(&mut providers.value, system_voice, legacy_shape);
        if providers.schema_version < SETTINGS_SCHEMA_VERSION || providers.value != before {
            write_document(connection, "providers.model", "default", &providers.value)?;
        }
    }
    if let Some(mut routing) = read_document(connection, "routing.tasks", "default")? {
        let before = routing.value.clone();
        migrate_routing_document(&mut routing.value);
        if routing.schema_version < SETTINGS_SCHEMA_VERSION || routing.value != before {
            write_document(connection, "routing.tasks", "default", &routing.value)?;
        }
    }
    if let Some(voice) = old_voice {
        let migrated = migrated_voice_document(&voice.value);
        if voice.schema_version < SETTINGS_SCHEMA_VERSION || migrated != voice.value {
            write_document(connection, "voice.runtime", "default", &migrated)?;
        }
    }
    if let Some(mut security) = read_document(connection, "security.runtime", "default")? {
        let before = security.value.clone();
        migrate_security_document(&mut security.value);
        if security.schema_version < SETTINGS_SCHEMA_VERSION || security.value != before {
            write_document(connection, "security.runtime", "default", &security.value)?;
        }
    }
    if let Some(mut agent) = read_document(connection, "providers.agent", "codex-sdk")? {
        let before = agent.value.clone();
        migrate_agent_document(&mut agent.value);
        if agent.schema_version < SETTINGS_SCHEMA_VERSION || agent.value != before {
            write_document(connection, "providers.agent", "codex-sdk", &agent.value)?;
        }
    }
    connection.execute(
        "UPDATE settings_documents SET schema_version=?1, updated_at=?2 WHERE schema_version < ?1",
        params![SETTINGS_SCHEMA_VERSION, now_iso()],
    )?;
    Ok(())
}

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

        let voice = migrated_voice_document(&json!({
            "inputDeviceId": "mic", "captureMode": "push-to-talk", "ttsVoice": "Kyoko"
        }));
        assert_eq!(voice["listeningEnabled"], true);
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
}
