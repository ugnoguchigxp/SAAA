use rusqlite::{params, Connection};
use serde_json::{json, Value};

mod provider_validation;
pub(crate) mod regional_preferences;
pub(crate) use provider_validation::validate_model_providers;

pub(crate) const SETTINGS_SCHEMA_VERSION: i64 = 13;
const DEFAULT_CONVERSATION_TIMEOUT_MS: u64 = 1_800_000;
const MAX_CONVERSATION_TIMEOUT_MS: u64 = 3_600_000;

use crate::{
    database_error, now_iso, providers, situation, CodexAgentRuntimeSettings,
    ModelProviderSettings, ModelProvidersSettings, RoutingSettings, SaveSettingsDocumentInput,
    SecurityRuntimeSettings, SettingsDocument, VoiceRuntimeSettings, DEFAULT_AGENT_NAME,
    DEFAULT_DYNAMIC_LAN_HOST, DEFAULT_USER_NAME, DYNAMIC_LAN_PROVIDER_ID,
};

pub(crate) fn load_codex_settings(
    connection: &Connection,
) -> Result<CodexAgentRuntimeSettings, String> {
    let document = read_settings_document(connection, "providers.agent", "codex-sdk")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode Codex settings: {error}"))?;
    validate_codex_settings(&settings)?;
    Ok(settings)
}

pub(crate) fn load_model_providers(
    connection: &Connection,
) -> Result<ModelProvidersSettings, String> {
    let document = read_settings_document(connection, "providers.model", "default")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode provider settings: {error}"))?;
    validate_model_providers(&settings)?;
    Ok(settings)
}

pub(crate) fn load_voice_settings(connection: &Connection) -> Result<VoiceRuntimeSettings, String> {
    let document = read_settings_document(connection, "voice.runtime", "default")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode voice settings: {error}"))?;
    validate_voice_settings(&settings)?;
    Ok(settings)
}

pub(crate) fn set_voice_listening_enabled_to_connection(
    connection: &Connection,
    enabled: bool,
) -> Result<SettingsDocument, String> {
    let mut document = read_settings_document(connection, "voice.runtime", "default")?;
    let mut settings = serde_json::from_value::<VoiceRuntimeSettings>(document.value_json.clone())
        .map_err(|error| format!("Could not decode voice settings: {error}"))?;
    settings.listening_enabled = enabled;
    validate_voice_settings(&settings)?;
    document.value_json = serde_json::to_value(settings)
        .map_err(|error| format!("Could not encode voice settings: {error}"))?;
    let value_text = serde_json::to_string(&document.value_json)
        .map_err(|error| format!("Could not encode voice settings: {error}"))?;
    connection
        .execute(
            "UPDATE settings_documents
             SET schema_version=?1, value_json=?2, updated_at=?3
             WHERE namespace='voice.runtime' AND key='default'",
            params![SETTINGS_SCHEMA_VERSION, value_text, now_iso()],
        )
        .map_err(database_error)?;
    read_settings_document(connection, "voice.runtime", "default")
}

pub(crate) fn set_voice_listening_enabled(
    state: &crate::AppState,
    enabled: bool,
) -> Result<SettingsDocument, String> {
    let connection = state
        .connection
        .lock()
        .map_err(|_| "Database lock unavailable".to_string())?;
    set_voice_listening_enabled_to_connection(&connection, enabled)
}

pub(crate) fn load_routing_settings(connection: &Connection) -> Result<RoutingSettings, String> {
    let document = read_settings_document(connection, "routing.tasks", "default")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode route settings: {error}"))?;
    validate_routing_settings(&settings)?;
    Ok(settings)
}

pub(crate) fn load_security_settings(
    connection: &Connection,
) -> Result<SecurityRuntimeSettings, String> {
    let document = read_settings_document(connection, "security.runtime", "default")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode security settings: {error}"))?;
    validate_security_settings(&settings)?;
    Ok(settings)
}

pub(crate) fn save_settings_documents_to_connection(
    connection: &mut Connection,
    documents: &[SaveSettingsDocumentInput],
) -> Result<Vec<SettingsDocument>, String> {
    if documents.is_empty() {
        return Err("No settings documents to save".to_string());
    }
    for document in documents {
        validate_settings_document(document)?;
    }
    validate_settings_batch(documents)?;
    let transaction = connection.transaction().map_err(database_error)?;
    for document in documents {
        let value_text = serde_json::to_string(&document.value_json)
            .map_err(|error| format!("Could not encode settings: {error}"))?;
        transaction
            .execute(
                "INSERT INTO settings_documents(namespace, key, schema_version, value_json, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(namespace, key) DO UPDATE SET
                   schema_version = excluded.schema_version,
                   value_json = excluded.value_json,
                   updated_at = excluded.updated_at",
                params![
                    document.namespace,
                    document.key,
                    document.schema_version,
                    value_text,
                    now_iso()
                ],
            )
            .map_err(database_error)?;
    }
    transaction.commit().map_err(database_error)?;

    let saved = documents
        .iter()
        .map(|document| read_settings_document(connection, &document.namespace, &document.key))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(saved)
}

pub(crate) fn default_settings_documents() -> Vec<(&'static str, &'static str, i64, Value)> {
    vec![
        (
            "providers.model",
            "default",
            SETTINGS_SCHEMA_VERSION,
            json!({
                "harness": {
                    "address": format!("http://{}:9810", DEFAULT_DYNAMIC_LAN_HOST)
                },
                "providers": [{
                    "kind": "dynamic-lan",
                    "id": DYNAMIC_LAN_PROVIDER_ID,
                    "enabled": true,
                    "label": "Provider Harness LLM",
                    "location": "local",
                    "host": DEFAULT_DYNAMIC_LAN_HOST
                }, {
                    "kind": "system-tts",
                    "id": "system-tts",
                    "enabled": true,
                    "label": "System Voice",
                    "location": "local",
                    "voice": "default"
                }],
                "reasoningEffort": providers::DEFAULT_CONVERSATION_REASONING_EFFORT
            }),
        ),
        (
            "providers.agent",
            "codex-sdk",
            SETTINGS_SCHEMA_VERSION,
            json!({
                "agentName": DEFAULT_AGENT_NAME,
                "userName": DEFAULT_USER_NAME,
                "enabled": false,
                "provider": "codex-sdk",
                "model": "",
                "runtimeMode": "app-server",
                "health": "unchecked",
                "sandboxMode": "read-only",
                "approvalPolicy": "never",
                "networkEnabled": false,
                "webSearchEnabled": false,
                "workspacePolicy": "select-per-conversation"
            }),
        ),
        (
            "routing.tasks",
            "default",
            SETTINGS_SCHEMA_VERSION,
            json!({
                "conversationRespond": {
                    "source": "harness",
                    "primaryProviderId": null,
                    "fallbackProviderIds": [],
                    "timeoutMs": DEFAULT_CONVERSATION_TIMEOUT_MS
                },
                "voiceTranscribe": {
                    "source": "harness",
                    "providerId": null,
                    "timeoutMs": 120000
                },
                "voiceSpeak": {
                    "source": "provider",
                    "providerId": "system-tts",
                    "timeoutMs": 30000
                },
                "codingAssist": {
                    "providerId": "codex-sdk",
                    "timeoutMs": 120000,
                    "readOnly": true,
                    "networkEnabled": false,
                    "webSearchEnabled": false
                }
            }),
        ),
        (
            "voice.runtime",
            "default",
            SETTINGS_SCHEMA_VERSION,
            json!({
                "listeningEnabled": false,
                "inputDeviceId": "default",
                "outputDeviceId": "default",
                "vadSensitivity": "medium",
                "silenceTimeoutMs": 1500,
                "allowedLanguages": [crate::voice::language::DEFAULT_LANGUAGE_CODE],
                "autoSpeak": true
            }),
        ),
        (
            "security.runtime",
            "default",
            SETTINGS_SCHEMA_VERSION,
            json!({
                "localOnlyWhenSelected": true,
                "diagnosticsRedaction": true
            }),
        ),
        (
            "ui.preferences",
            "default",
            SETTINGS_SCHEMA_VERSION,
            regional_preferences::default_value(),
        ),
        (
            "situation.runtime",
            "default",
            SETTINGS_SCHEMA_VERSION,
            serde_json::to_value(situation::contracts::SituationRuntimeSettings::default())
                .expect("default Situation settings serialize"),
        ),
    ]
}

pub(crate) fn validate_settings_document(input: &SaveSettingsDocumentInput) -> Result<(), String> {
    let allowed = matches!(
        (input.namespace.as_str(), input.key.as_str()),
        ("providers.model", "default")
            | ("providers.agent", "codex-sdk")
            | ("routing.tasks", "default")
            | ("voice.runtime", "default")
            | ("security.runtime", "default")
            | ("ui.preferences", "default")
            | ("situation.runtime", "default")
    );
    if !allowed {
        return Err("Unsupported settings document".to_string());
    }
    if input.schema_version != SETTINGS_SCHEMA_VERSION || !input.value_json.is_object() {
        return Err("Invalid settings schema".to_string());
    }
    match (input.namespace.as_str(), input.key.as_str()) {
        ("providers.model", "default") => {
            let settings =
                serde_json::from_value::<ModelProvidersSettings>(input.value_json.clone())
                    .map_err(|error| format!("Invalid model provider settings: {error}"))?;
            validate_model_providers(&settings)
        }
        ("providers.agent", "codex-sdk") => {
            let settings =
                serde_json::from_value::<CodexAgentRuntimeSettings>(input.value_json.clone())
                    .map_err(|error| format!("Invalid Codex settings: {error}"))?;
            validate_codex_settings(&settings)
        }
        ("routing.tasks", "default") => {
            let settings = serde_json::from_value::<RoutingSettings>(input.value_json.clone())
                .map_err(|error| format!("Invalid routing settings: {error}"))?;
            validate_routing_settings(&settings)
        }
        ("voice.runtime", "default") => {
            let settings = serde_json::from_value::<VoiceRuntimeSettings>(input.value_json.clone())
                .map_err(|error| format!("Invalid voice settings: {error}"))?;
            validate_voice_settings(&settings)
        }
        ("security.runtime", "default") => {
            let settings =
                serde_json::from_value::<SecurityRuntimeSettings>(input.value_json.clone())
                    .map_err(|error| format!("Invalid security settings: {error}"))?;
            validate_security_settings(&settings)
        }
        ("ui.preferences", "default") => regional_preferences::validate(input.value_json.clone()),
        ("situation.runtime", "default") => {
            let settings =
                serde_json::from_value::<situation::contracts::SituationRuntimeSettings>(
                    input.value_json.clone(),
                )
                .map_err(|error| format!("Invalid Situation settings: {error}"))?;
            situation::validate_settings(&settings)
        }
        _ => Err("Unsupported settings document".to_string()),
    }
}

pub(crate) fn validate_settings_batch(
    documents: &[SaveSettingsDocumentInput],
) -> Result<(), String> {
    if documents.len() != 7 {
        return Err("A complete seven-document settings snapshot is required".to_string());
    }
    let unique = documents
        .iter()
        .map(|document| (document.namespace.as_str(), document.key.as_str()))
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != 7 {
        return Err("Each settings document must appear exactly once".to_string());
    }
    let providers = documents
        .iter()
        .find(|document| document.namespace == "providers.model" && document.key == "default")
        .ok_or_else(|| "Model provider settings are required".to_string())?;
    let routing = documents
        .iter()
        .find(|document| document.namespace == "routing.tasks" && document.key == "default")
        .ok_or_else(|| "Routing settings are required".to_string())?;
    let security = documents
        .iter()
        .find(|document| document.namespace == "security.runtime" && document.key == "default")
        .ok_or_else(|| "Security settings are required".to_string())?;
    let providers = serde_json::from_value::<ModelProvidersSettings>(providers.value_json.clone())
        .map_err(|error| format!("Invalid model provider settings: {error}"))?;
    let routing = serde_json::from_value::<RoutingSettings>(routing.value_json.clone())
        .map_err(|error| format!("Invalid routing settings: {error}"))?;
    let security = serde_json::from_value::<SecurityRuntimeSettings>(security.value_json.clone())
        .map_err(|error| format!("Invalid security settings: {error}"))?;
    let uses_harness = routing.conversation_respond.source == "harness"
        || routing.voice_transcribe.source == "harness"
        || routing.voice_speak.source == "harness";
    if uses_harness && providers.harness.address.trim().is_empty() {
        return Err(
            "Provider Harness address is required while a Harness route is selected".to_string(),
        );
    }
    let enabled_provider = |provider_id: &str| {
        providers
            .providers
            .iter()
            .find(|provider| provider.id() == provider_id && provider.enabled())
    };
    let conversation = &routing.conversation_respond;
    let primary_id = conversation.primary_provider_id.as_deref();
    if conversation.source == "provider" {
        let primary = primary_id
            .and_then(enabled_provider)
            .ok_or_else(|| "The individual conversation provider must be enabled".to_string())?;
        if !matches!(
            primary,
            ModelProviderSettings::OpenAiCompatible(_)
                | ModelProviderSettings::Larm(_)
                | ModelProviderSettings::DynamicLan(_)
        ) {
            return Err("The selected conversation provider does not support LLM".to_string());
        }
    }
    let primary_is_dynamic_lan = primary_id.is_some_and(|primary_id| {
        providers.providers.iter().any(|provider| {
            provider.id() == primary_id && matches!(provider, ModelProviderSettings::DynamicLan(_))
        })
    });
    if primary_is_dynamic_lan
        && conversation.timeout_ms > providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS
    {
        return Err(format!(
            "dynamic LAN conversation timeout must not exceed {} ms",
            providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS
        ));
    }
    let mut route_ids = std::collections::HashSet::new();
    if let Some(primary_id) = primary_id {
        route_ids.insert(primary_id);
    }
    for provider_id in &conversation.fallback_provider_ids {
        let fallback = enabled_provider(provider_id)
            .ok_or_else(|| format!("Fallback provider is not enabled: {provider_id}"))?;
        if !matches!(
            fallback,
            ModelProviderSettings::OpenAiCompatible(_)
                | ModelProviderSettings::Larm(_)
                | ModelProviderSettings::DynamicLan(_)
        ) {
            return Err(format!("Fallback provider is not enabled: {provider_id}"));
        }
        if !route_ids.insert(provider_id) {
            return Err(format!("Duplicate provider in route: {provider_id}"));
        }
        let primary_is_local = primary_id.is_some_and(|primary_id| {
            providers
                .providers
                .iter()
                .any(|provider| provider.id() == primary_id && provider.location() == "local")
        });
        let fallback_is_cloud = providers
            .providers
            .iter()
            .any(|provider| provider.id() == *provider_id && provider.location() == "cloud");
        if security.local_only_when_selected && primary_is_local && fallback_is_cloud {
            return Err(format!(
                "Cloud fallback is blocked while the local-only policy is active: {provider_id}"
            ));
        }
    }
    if conversation.source == "harness" && !conversation.fallback_provider_ids.is_empty() {
        return Err("Harness routes do not use individual provider fallbacks".to_string());
    }
    validate_voice_route_provider(
        &routing.voice_transcribe.source,
        routing.voice_transcribe.provider_id.as_deref(),
        &providers.providers,
        |provider| matches!(provider, ModelProviderSettings::CloudAsr(_)),
        "ASR",
    )?;
    validate_voice_route_provider(
        &routing.voice_speak.source,
        routing.voice_speak.provider_id.as_deref(),
        &providers.providers,
        |provider| {
            matches!(
                provider,
                ModelProviderSettings::CloudTts(_) | ModelProviderSettings::SystemTts(_)
            )
        },
        "TTS",
    )?;
    Ok(())
}

fn validate_voice_route_provider(
    source: &str,
    provider_id: Option<&str>,
    providers: &[ModelProviderSettings],
    supports_capability: impl Fn(&ModelProviderSettings) -> bool,
    capability: &str,
) -> Result<(), String> {
    if source == "harness" {
        return (provider_id.is_none())
            .then_some(())
            .ok_or_else(|| format!("Harness {capability} route must not reference a provider"));
    }
    let provider = provider_id
        .and_then(|provider_id| {
            providers
                .iter()
                .find(|provider| provider.id() == provider_id && provider.enabled())
        })
        .ok_or_else(|| format!("The individual {capability} provider must be enabled"))?;
    supports_capability(provider)
        .then_some(())
        .ok_or_else(|| format!("The selected provider does not support {capability}"))
}

pub(crate) fn validate_codex_settings(settings: &CodexAgentRuntimeSettings) -> Result<(), String> {
    if settings.agent_name.trim().is_empty()
        || settings.agent_name.trim() != settings.agent_name
        || settings.agent_name.chars().count() > 80
        || settings.agent_name.chars().any(char::is_control)
        || settings.user_name.trim() != settings.user_name
        || settings.user_name.chars().count() > 80
        || settings.user_name.chars().any(char::is_control)
        || settings.provider != "codex-sdk"
        || !matches!(
            settings.runtime_mode.as_str(),
            "pending-compatibility-check" | "bun" | "node-sidecar" | "app-server"
        )
        || !matches!(
            settings.health.as_str(),
            "unchecked" | "ready" | "unavailable"
        )
        || settings.sandbox_mode != "read-only"
        || settings.approval_policy != "never"
        || settings.network_enabled
        || settings.web_search_enabled
        || settings.workspace_policy != "select-per-conversation"
        || settings.model.chars().count() > 160
        || settings.model.chars().any(char::is_control)
    {
        return Err("Codex settings violate the fixed read-only policy".to_string());
    }
    Ok(())
}

pub(crate) fn validate_routing_settings(settings: &RoutingSettings) -> Result<(), String> {
    let conversation = &settings.conversation_respond;
    let transcribe = &settings.voice_transcribe;
    let speak = &settings.voice_speak;
    let coding = &settings.coding_assist;
    let valid_provider_id = |provider_id: &str| {
        !provider_id.is_empty()
            && provider_id.len() <= 80
            && provider_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
    };
    let valid_source = |source: &str, provider_id: Option<&str>| match source {
        "harness" => provider_id.is_none(),
        "provider" => provider_id.is_some_and(valid_provider_id),
        _ => false,
    };
    if !valid_source(
        &conversation.source,
        conversation.primary_provider_id.as_deref(),
    ) || conversation.fallback_provider_ids.len() > 20
        || conversation
            .fallback_provider_ids
            .iter()
            .any(|provider_id| !valid_provider_id(provider_id))
        || !(1_000..=MAX_CONVERSATION_TIMEOUT_MS).contains(&conversation.timeout_ms)
        || !valid_source(&transcribe.source, transcribe.provider_id.as_deref())
        || !(1_000..=300_000).contains(&transcribe.timeout_ms)
        || !valid_source(&speak.source, speak.provider_id.as_deref())
        || !(1_000..=300_000).contains(&speak.timeout_ms)
        || coding.provider_id != "codex-sdk"
        || !(1_000..=300_000).contains(&coding.timeout_ms)
        || !coding.read_only
        || coding.network_enabled
        || coding.web_search_enabled
    {
        return Err("Invalid task routing settings".to_string());
    }
    Ok(())
}

pub(crate) fn validate_voice_settings(settings: &VoiceRuntimeSettings) -> Result<(), String> {
    if settings.input_device_id.trim().is_empty()
        || settings.input_device_id.len() > 300
        || settings.output_device_id.trim().is_empty()
        || settings.output_device_id.len() > 300
        || !matches!(settings.vad_sensitivity.as_str(), "low" | "medium" | "high")
        || !(800..=3_000).contains(&settings.silence_timeout_ms)
        || crate::voice::language::validate_allowed_languages(&settings.allowed_languages).is_err()
    {
        return Err("Invalid continuous listening settings".to_string());
    }
    Ok(())
}

pub(crate) fn validate_security_settings(settings: &SecurityRuntimeSettings) -> Result<(), String> {
    if !settings.diagnostics_redaction {
        return Err("Diagnostics must remain redacted".to_string());
    }
    let _ = settings.local_only_when_selected;
    Ok(())
}

pub(crate) fn read_settings_document(
    connection: &Connection,
    namespace: &str,
    key: &str,
) -> Result<SettingsDocument, String> {
    let document = connection
        .query_row(
            "SELECT namespace, key, schema_version, value_json, updated_at
             FROM settings_documents WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
            settings_document_from_row,
        )
        .map_err(database_error)?;
    validate_stored_settings_document(&document)?;
    Ok(document)
}

pub(crate) fn list_settings_documents(
    connection: &Connection,
) -> Result<Vec<SettingsDocument>, String> {
    let mut statement = connection
        .prepare(
            "SELECT namespace, key, schema_version, value_json, updated_at
             FROM settings_documents ORDER BY namespace, key",
        )
        .map_err(database_error)?;
    let documents = statement
        .query_map([], settings_document_from_row)
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let inputs = documents
        .iter()
        .map(|document| SaveSettingsDocumentInput {
            namespace: document.namespace.clone(),
            key: document.key.clone(),
            schema_version: document.schema_version,
            value_json: document.value_json.clone(),
        })
        .collect::<Vec<_>>();
    for input in &inputs {
        validate_settings_document(input)?;
    }
    validate_settings_batch(&inputs)?;
    Ok(documents)
}

pub(crate) fn validate_stored_settings_document(document: &SettingsDocument) -> Result<(), String> {
    validate_settings_document(&SaveSettingsDocumentInput {
        namespace: document.namespace.clone(),
        key: document.key.clone(),
        schema_version: document.schema_version,
        value_json: document.value_json.clone(),
    })
}

pub(crate) fn settings_document_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SettingsDocument> {
    let value_text: String = row.get(3)?;
    let value_json = serde_json::from_str(&value_text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(SettingsDocument {
        namespace: row.get(0)?,
        key: row.get(1)?,
        schema_version: row.get(2)?,
        value_json,
        updated_at: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        default_settings_input, direct_provider, dynamic_lan_provider, larm_provider, provider,
    };
    use crate::{
        initialize_database, providers, CodexAgentRuntimeSettings, ModelProviderSettings,
        ModelProvidersSettings, OpenAiCompatibleProviderSettings, DYNAMIC_LAN_PROVIDER_ID,
    };
    use rusqlite::Connection;
    use serde_json::{json, Value};

    #[test]
    fn defaults_conversation_timeout_to_thirty_minutes() {
        let documents = default_settings_documents();
        let routing = documents
            .iter()
            .find(|(namespace, key, _, _)| *namespace == "routing.tasks" && *key == "default")
            .expect("default routing settings");
        assert_eq!(
            routing.3.pointer("/conversationRespond/timeoutMs"),
            Some(&json!(1_800_000))
        );
    }

    #[test]
    fn ambient_listening_requires_consent_and_persists_immediately() {
        let connection = Connection::open_in_memory().expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let defaults = load_voice_settings(&connection).expect("voice settings load");
        assert!(!defaults.listening_enabled);

        let enabled = set_voice_listening_enabled_to_connection(&connection, true)
            .expect("listening preference saves");
        assert_eq!(enabled.value_json["listeningEnabled"], true);
        assert_eq!(enabled.schema_version, SETTINGS_SCHEMA_VERSION);
        assert!(
            load_voice_settings(&connection)
                .expect("updated voice settings load")
                .listening_enabled
        );
    }

    #[test]
    fn accepts_conversation_timeout_up_to_one_hour() {
        let mut routing = serde_json::from_value::<RoutingSettings>(
            default_settings_documents()
                .into_iter()
                .find(|(namespace, key, _, _)| *namespace == "routing.tasks" && *key == "default")
                .expect("default routing settings")
                .3,
        )
        .expect("routing settings decode");
        routing.conversation_respond.timeout_ms = MAX_CONVERSATION_TIMEOUT_MS;
        assert!(validate_routing_settings(&routing).is_ok());
        routing.conversation_respond.timeout_ms = MAX_CONVERSATION_TIMEOUT_MS + 1;
        assert!(validate_routing_settings(&routing).is_err());
    }

    #[test]
    fn settings_survive_reopen_and_invalid_batch_does_not_replace_snapshot() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("settings.sqlite3");
        let mut connection = Connection::open(&path).expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let mut documents = default_settings_input();
        let security = documents
            .iter_mut()
            .find(|document| document.namespace == "security.runtime")
            .expect("security document");
        security.value_json["localOnlyWhenSelected"] = Value::Bool(false);
        let routing = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing document");
        routing.value_json["conversationRespond"]["timeoutMs"] = json!(1_234_000);
        save_settings_documents_to_connection(&mut connection, &documents)
            .expect("valid settings save");
        drop(connection);

        let mut reopened = Connection::open(&path).expect("database reopens");
        initialize_database(&reopened).expect("database reinitializes");
        let loaded = list_settings_documents(&reopened).expect("settings reload");
        let security = loaded
            .iter()
            .find(|document| document.namespace == "security.runtime")
            .expect("security settings reload");
        assert_eq!(security.value_json["localOnlyWhenSelected"], false);
        let routing = loaded
            .iter()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing settings reload");
        assert_eq!(
            routing.value_json["conversationRespond"]["timeoutMs"],
            1_234_000
        );

        let mut invalid = default_settings_input();
        invalid
            .iter_mut()
            .find(|document| document.namespace == "security.runtime")
            .expect("security document")
            .value_json["credentialStorage"] = Value::String("plaintext".to_string());
        assert!(save_settings_documents_to_connection(&mut reopened, &invalid).is_err());
        let unchanged = read_settings_document(&reopened, "security.runtime", "default")
            .expect("previous snapshot remains");
        assert_eq!(unchanged.value_json["localOnlyWhenSelected"], false);
    }

    #[test]
    fn settings_reject_embedded_credentials_and_cloud_fallback_on_local_route() {
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![provider("local", "local")],
            reasoning_effort: "mid".to_string(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string()
            },
        })
        .is_err());
        let mut dynamic_lan = direct_provider(DYNAMIC_LAN_PROVIDER_ID, "local");
        dynamic_lan.endpoint = "http://10.0.0.42:8083/v1".to_string();
        dynamic_lan.model = "ornith15-35b".to_string();
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(dynamic_lan)],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string()
            },
        })
        .is_ok());
        let mut public_http = direct_provider("public-http", "local");
        public_http.endpoint = "http://203.0.113.10:8080/v1".to_string();
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(public_http)],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string()
            },
        })
        .is_err());

        let with_credentials = ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(
                OpenAiCompatibleProviderSettings {
                    endpoint: "https://user:secret@example.invalid/v1".to_string(),
                    ..direct_provider("cloud", "cloud")
                },
            )],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
        };
        assert!(validate_model_providers(&with_credentials).is_err());
        let mut disabled_with_credentials = with_credentials;
        let ModelProviderSettings::OpenAiCompatible(disabled_provider) =
            &mut disabled_with_credentials.providers[0]
        else {
            unreachable!("fixture is direct provider");
        };
        disabled_provider.enabled = false;
        assert!(validate_model_providers(&disabled_with_credentials).is_err());

        let unsafe_id = ModelProvidersSettings {
            providers: vec![provider("local provider", "local")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
        };
        assert!(validate_model_providers(&unsafe_id).is_err());
        let ambiguous_ids = ModelProvidersSettings {
            providers: vec![provider("local-a", "local"), provider("local_a", "local")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
        };
        assert!(validate_model_providers(&ambiguous_ids).is_ok());

        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings")
            .value_json = json!({
            "harness": { "address": "http://localhost:9810" },
            "providers": [provider("local", "local"), provider("cloud", "cloud")],
            "reasoningEffort": "medium"
        });
        let routing = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        routing.value_json["conversationRespond"]["source"] = json!("provider");
        routing.value_json["conversationRespond"]["primaryProviderId"] = json!("local");
        routing.value_json["conversationRespond"]["fallbackProviderIds"] = json!(["cloud"]);
        assert!(validate_settings_batch(&documents).is_err());

        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings")
            .value_json = json!({
            "harness": { "address": "http://localhost:9810" },
            "providers": [
                dynamic_lan_provider("dynamic_lan-primary"),
                provider("local-fallback", "local")
            ],
            "reasoningEffort": "medium"
        });
        let routing_index = documents
            .iter()
            .position(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        documents[routing_index].value_json["conversationRespond"]["primaryProviderId"] =
            json!("dynamic_lan-primary");
        documents[routing_index].value_json["conversationRespond"]["source"] = json!("provider");
        documents[routing_index].value_json["conversationRespond"]["timeoutMs"] = json!(30_000);
        documents[routing_index].value_json["voiceSpeak"]["source"] = json!("harness");
        documents[routing_index].value_json["voiceSpeak"]["providerId"] = Value::Null;
        documents[routing_index].value_json["conversationRespond"]["fallbackProviderIds"] =
            json!(["local-fallback"]);
        assert!(validate_settings_batch(&documents).is_ok());

        documents[routing_index].value_json["conversationRespond"]["fallbackProviderIds"] =
            json!([]);
        documents[routing_index].value_json["conversationRespond"]["timeoutMs"] =
            json!(providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS + 1);
        assert!(validate_settings_batch(&documents).is_err());
        documents[routing_index].value_json["conversationRespond"]["timeoutMs"] =
            json!(providers::dynamic_lan::MAX_REQUEST_TIMEOUT_MS);
        assert!(validate_settings_batch(&documents).is_ok());
    }

    #[test]
    fn harness_routes_require_a_nonempty_address() {
        let mut documents = default_settings_input();
        let provider_settings = documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings");
        provider_settings.value_json["harness"]["address"] = json!("");
        assert!(validate_settings_batch(&documents).is_err());
    }

    #[test]
    fn larm_settings_enforce_the_fixed_loopback_security_contract() {
        let valid = ModelProvidersSettings {
            providers: vec![larm_provider("larm")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
        };
        assert!(validate_model_providers(&valid).is_ok());
        let mut ipv6 = larm_provider("larm-ipv6");
        let ModelProviderSettings::Larm(provider) = &mut ipv6 else {
            unreachable!("LARM fixture must remain tagged as LARM");
        };
        provider.base_url = "http://[::1]:9810/".to_string();
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ipv6],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string()
            },
        })
        .is_ok());

        for base_url in [
            "http://localhost:9810/",
            "http://192.168.1.20:9810/",
            "https://127.0.0.1:9810/",
            "http://127.0.0.1:9810/v1",
            "http://user:secret@127.0.0.1:9810/",
            "http://127.0.0.1/",
        ] {
            let mut invalid = larm_provider("larm");
            let ModelProviderSettings::Larm(provider) = &mut invalid else {
                unreachable!("LARM fixture must remain tagged as LARM");
            };
            provider.base_url = base_url.to_string();
            assert!(
                validate_model_providers(&ModelProvidersSettings {
                    providers: vec![invalid],
                    reasoning_effort: providers::default_conversation_reasoning_effort(),
                    harness: crate::HarnessSettings {
                        address: "http://localhost:9810".to_string()
                    },
                })
                .is_err(),
                "invalid LARM URL was accepted: {base_url}"
            );
        }

        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![larm_provider("larm-a"), larm_provider("larm-b")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string()
            },
        })
        .is_err());
    }

    #[test]
    fn legacy_provider_ids_and_default_codex_model_remain_valid() {
        let providers = ModelProvidersSettings {
            providers: vec![provider("Local_Custom", "local")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
        };
        assert!(validate_model_providers(&providers).is_ok());

        let codex = CodexAgentRuntimeSettings {
            agent_name: DEFAULT_AGENT_NAME.to_string(),
            user_name: DEFAULT_USER_NAME.to_string(),
            enabled: true,
            provider: "codex-sdk".to_string(),
            model: String::new(),
            runtime_mode: "app-server".to_string(),
            health: "unchecked".to_string(),
            sandbox_mode: "read-only".to_string(),
            approval_policy: "never".to_string(),
            network_enabled: false,
            web_search_enabled: false,
            workspace_policy: "select-per-conversation".to_string(),
        };
        assert!(validate_codex_settings(&codex).is_ok());
    }

    #[test]
    fn human_readable_setting_limits_count_characters_not_utf8_bytes() {
        let mut provider = direct_provider("localized", "local");
        provider.label = "あ".repeat(120);
        provider.model = "モ".repeat(160);
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(provider.clone())],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string()
            },
        })
        .is_ok());

        provider.label.push('あ');
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(provider)],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string()
            },
        })
        .is_err());

        let mut documents = default_settings_input();
        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["inputDeviceId"] = json!("声".repeat(100));
        validate_settings_document(voice).expect("bounded localized device id is accepted");
        voice.value_json["inputDeviceId"] = json!("声".repeat(101));
        assert!(validate_settings_document(voice).is_err());
    }

    #[test]
    fn voice_settings_require_bounded_continuous_listening_options() {
        let mut documents = default_settings_input();
        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["vadSensitivity"] = json!("maximum");
        assert_eq!(
            validate_settings_document(voice).expect_err("unknown sensitivity is rejected"),
            "Invalid continuous listening settings"
        );

        voice.value_json["vadSensitivity"] = json!("medium");
        voice.value_json["silenceTimeoutMs"] = json!(3_001);
        assert!(validate_settings_document(voice).is_err());
    }

    #[test]
    fn voice_settings_require_registered_supported_languages() {
        let mut documents = default_settings_input();
        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["allowedLanguages"] = json!(["ja", "en"]);
        validate_settings_document(voice).expect("supported languages are accepted");

        for invalid in [json!([]), json!(["xx"]), json!(["ja", "ja"])] {
            voice.value_json["allowedLanguages"] = invalid;
            assert_eq!(
                validate_settings_document(voice).expect_err("invalid languages are rejected"),
                "Invalid continuous listening settings"
            );
        }
    }
}
