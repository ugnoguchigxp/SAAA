use super::*;
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
