const STRICT_PROVIDER_AND_VOICE_SHAPE_VERSION: i64 = 13;
pub(super) fn initialize_revision(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings_revision (
           singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
           revision INTEGER NOT NULL CHECK(revision >= 0));
         INSERT OR IGNORE INTO settings_revision(singleton, revision) VALUES(1, 0);
         CREATE TRIGGER IF NOT EXISTS settings_revision_insert AFTER INSERT ON settings_documents
           BEGIN UPDATE settings_revision SET revision = revision + 1 WHERE singleton = 1; END;
         CREATE TRIGGER IF NOT EXISTS settings_revision_update AFTER UPDATE ON settings_documents
           BEGIN UPDATE settings_revision SET revision = revision + 1 WHERE singleton = 1; END;
         CREATE TRIGGER IF NOT EXISTS settings_revision_delete AFTER DELETE ON settings_documents
           BEGIN UPDATE settings_revision SET revision = revision + 1 WHERE singleton = 1; END;",
    )
}
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
fn migrate_obsolete_direct_lan_route(providers: &Value, routing: &mut Value) {
    let Some(conversation) = routing
        .get_mut("conversationRespond")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if conversation.get("source").and_then(Value::as_str) != Some("provider") {
        return;
    }
    let Some(primary_id) = conversation
        .get("primaryProviderId")
        .and_then(Value::as_str)
    else {
        return;
    };
    let Some(items) = providers.get("providers").and_then(Value::as_array) else {
        return;
    };
    let Some(dynamic) = items.iter().find(|provider| {
        provider.get("id").and_then(Value::as_str) == Some(DYNAMIC_LAN_PROVIDER_ID)
            && provider.get("kind").and_then(Value::as_str) == Some("dynamic-lan")
            && provider.get("enabled").and_then(Value::as_bool) == Some(true)
    }) else {
        return;
    };
    let Some(dynamic_host) = dynamic.get("host").and_then(Value::as_str) else {
        return;
    };
    let harness_matches = providers
        .pointer("/harness/address")
        .and_then(Value::as_str)
        .and_then(|address| url::Url::parse(address).ok())
        .is_some_and(|url| {
            url.scheme() == "http"
                && url.host_str() == Some(dynamic_host)
                && url.port() == Some(crate::providers::dynamic_lan::CONTROL_PORT)
                && url.path() == "/"
        });
    let direct_matches = items
        .iter()
        .find(|provider| provider.get("id").and_then(Value::as_str) == Some(primary_id))
        .filter(|provider| {
            provider.get("kind").and_then(Value::as_str) == Some("openai-compatible")
                && provider.get("location").and_then(Value::as_str) == Some("local")
                && provider.get("enabled").and_then(Value::as_bool) == Some(true)
        })
        .and_then(|provider| provider.get("endpoint").and_then(Value::as_str))
        .and_then(|endpoint| url::Url::parse(endpoint).ok())
        .is_some_and(|url| {
            url.scheme() == "http"
                && url.host_str() == Some(dynamic_host)
                && url.port_or_known_default() == Some(8_080)
        });
    if harness_matches && direct_matches {
        conversation.insert("source".to_string(), Value::String("harness".to_string()));
        conversation.insert("primaryProviderId".to_string(), Value::Null);
        conversation.insert("fallbackProviderIds".to_string(), json!([]));
    }
}
fn migrated_voice_document(value: &Value, require_fresh_consent: bool) -> Value {
    json!({
        "listeningEnabled": if require_fresh_consent { false } else { value.get("listeningEnabled").and_then(Value::as_bool).unwrap_or(false) },
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
fn remove_direct_qwen_provider(providers: &mut Value) {
    let Some(items) = providers.get_mut("providers").and_then(Value::as_array_mut) else {
        return;
    };
    items.retain(|item| {
        item.get("id").and_then(Value::as_str) != Some(crate::QWEN_DIRECT_PROVIDER_ID)
    });
}

fn clear_direct_qwen_route(routing: &mut Value) {
    let Some(conversation) = routing
        .get_mut("conversationRespond")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    if conversation
        .get("primaryProviderId")
        .and_then(Value::as_str)
        == Some(crate::QWEN_DIRECT_PROVIDER_ID)
    {
        conversation.insert("source".to_string(), Value::String("harness".to_string()));
        conversation.insert("primaryProviderId".to_string(), Value::Null);
        conversation.insert("fallbackProviderIds".to_string(), json!([]));
        return;
    }
    if let Some(ids) = conversation
        .get_mut("fallbackProviderIds")
        .and_then(Value::as_array_mut)
    {
        ids.retain(|id| id.as_str() != Some(crate::QWEN_DIRECT_PROVIDER_ID));
    }
}

fn retarget_direct_qwen_actors(roles: &mut Value) {
    let Some(actors) = roles.get_mut("actors").and_then(Value::as_array_mut) else {
        return;
    };
    for actor in actors {
        if actor.get("providerId").and_then(Value::as_str) == Some(crate::QWEN_DIRECT_PROVIDER_ID) {
            actor["providerId"] = json!(DYNAMIC_LAN_PROVIDER_ID);
        }
    }
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
        let legacy_shape = providers.schema_version < STRICT_PROVIDER_AND_VOICE_SHAPE_VERSION
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
        if providers.schema_version < 15 {
            migrate_http_bases(&mut providers.value);
        }
        remove_direct_qwen_provider(&mut providers.value);

        if providers.schema_version < SETTINGS_SCHEMA_VERSION || providers.value != before {
            write_document(connection, "providers.model", "default", &providers.value)?;
        }
    }
    if let Some(mut routing) = read_document(connection, "routing.tasks", "default")? {
        let before = routing.value.clone();
        if routing.schema_version < 14 {
            if let Some(providers) = read_document(connection, "providers.model", "default")? {
                migrate_obsolete_direct_lan_route(&providers.value, &mut routing.value);
            }
        }
        clear_direct_qwen_route(&mut routing.value);
        migrate_routing_document(&mut routing.value);
        if routing.schema_version < SETTINGS_SCHEMA_VERSION || routing.value != before {
            write_document(connection, "routing.tasks", "default", &routing.value)?;
        }
    }
    if let Some(voice) = old_voice {
        let migrated = migrated_voice_document(
            &voice.value,
            voice.schema_version < STRICT_PROVIDER_AND_VOICE_SHAPE_VERSION,
        );
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
    if let Some(mut roles) = read_document(connection, "routing.roles", "default")? {
        let before = roles.value.clone();
        retarget_direct_qwen_actors(&mut roles.value);
        if roles.value != before {
            write_document(connection, "routing.roles", "default", &roles.value)?;
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
