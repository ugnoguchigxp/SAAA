use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::{
    database_error, now_iso, provider_environment_suffix, providers, situation, voice,
    CodexAgentRuntimeSettings, ModelProviderSettings, ModelProvidersSettings, RoutingSettings,
    SaveSettingsDocumentInput, SecurityRuntimeSettings, SettingsDocument, VoiceRuntimeSettings,
    GNOSIS_HOST, GNOSIS_PROVIDER_ID,
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
            9,
            json!({
                "providers": [{
                    "kind": "gnosis",
                    "id": GNOSIS_PROVIDER_ID,
                    "enabled": true,
                    "label": "gnosis · Dynamic LLM",
                    "location": "local",
                    "host": GNOSIS_HOST
                }],
                "reasoningEffort": providers::DEFAULT_CONVERSATION_REASONING_EFFORT
            }),
        ),
        (
            "providers.agent",
            "codex-sdk",
            9,
            json!({
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
            9,
            json!({
                "conversationRespond": {
                    "primaryProviderId": GNOSIS_PROVIDER_ID,
                    "fallbackProviderIds": [],
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
            9,
            json!({
                "inputDeviceId": "default",
                "outputDeviceId": "default",
                "captureMode": "push-to-talk",
                    "sttProviderId": "gnosis-asr",
                "sttModel": "qwen3-asr-1.7b",
                    "ttsProviderId": "system-tts",
                "ttsVoice": "default",
                "autoSpeak": true,
                "cloudFallbackEnabled": false
            }),
        ),
        (
            "security.runtime",
            "default",
            9,
            json!({
                "credentialStorage": "environment",
                "localOnlyWhenSelected": true,
                "diagnosticsRedaction": true
            }),
        ),
        (
            "situation.runtime",
            "default",
            9,
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
            | ("situation.runtime", "default")
    );
    if !allowed {
        return Err("Unsupported settings document".to_string());
    }
    if input.schema_version != 9 || !input.value_json.is_object() {
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
    if documents.len() != 6 {
        return Err("A complete six-document settings snapshot is required".to_string());
    }
    let unique = documents
        .iter()
        .map(|document| (document.namespace.as_str(), document.key.as_str()))
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != 6 {
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
    let enabled_ids = providers
        .providers
        .iter()
        .filter(|provider| provider.enabled())
        .map(ModelProviderSettings::id)
        .collect::<std::collections::HashSet<_>>();
    if !enabled_ids.is_empty()
        && !enabled_ids.contains(routing.conversation_respond.primary_provider_id.as_str())
    {
        return Err("The primary conversation provider must be enabled".to_string());
    }
    let primary_is_gnosis = providers.providers.iter().any(|provider| {
        provider.id() == routing.conversation_respond.primary_provider_id
            && matches!(provider, ModelProviderSettings::Gnosis(_))
    });
    if primary_is_gnosis
        && !routing
            .conversation_respond
            .fallback_provider_ids
            .is_empty()
    {
        return Err("gnosis routes must not configure fallback providers".to_string());
    }
    if primary_is_gnosis
        && routing.conversation_respond.timeout_ms > providers::gnosis::MAX_REQUEST_TIMEOUT_MS
    {
        return Err(format!(
            "gnosis conversation timeout must not exceed {} ms",
            providers::gnosis::MAX_REQUEST_TIMEOUT_MS
        ));
    }
    let mut route_ids = std::collections::HashSet::new();
    route_ids.insert(routing.conversation_respond.primary_provider_id.as_str());
    for provider_id in &routing.conversation_respond.fallback_provider_ids {
        if !enabled_ids.contains(provider_id.as_str()) {
            return Err(format!("Fallback provider is not enabled: {provider_id}"));
        }
        if !route_ids.insert(provider_id) {
            return Err(format!("Duplicate provider in route: {provider_id}"));
        }
        let primary_is_local = providers.providers.iter().any(|provider| {
            provider.id() == routing.conversation_respond.primary_provider_id
                && provider.location() == "local"
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
    Ok(())
}

pub(crate) fn validate_model_providers(settings: &ModelProvidersSettings) -> Result<(), String> {
    if !providers::valid_conversation_reasoning_effort(&settings.reasoning_effort) {
        return Err("Reasoning effort must be low, medium, or xhigh".to_string());
    }
    if settings.providers.is_empty() || settings.providers.len() > 20 {
        return Err("Between 1 and 20 model providers are required".to_string());
    }
    let mut ids = std::collections::HashSet::new();
    let mut credential_suffixes = std::collections::HashSet::new();
    let mut enabled_larm_count = 0;
    let mut enabled_gnosis_count = 0;
    for provider in &settings.providers {
        let provider_id = provider.id();
        if provider_id.is_empty()
            || provider_id.len() > 80
            || !provider_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
            || !ids.insert(provider_id)
            || !credential_suffixes.insert(provider_environment_suffix(provider_id))
        {
            return Err("Invalid, duplicate, or credential-ambiguous provider id".to_string());
        }
        if provider.label().trim().is_empty()
            || provider.label().chars().count() > 120
            || provider.label().chars().any(char::is_control)
        {
            return Err(format!("Invalid provider label: {provider_id}"));
        }
        if !matches!(provider.location(), "local" | "cloud") {
            return Err(format!("Invalid provider location: {provider_id}"));
        }
        match provider {
            ModelProviderSettings::OpenAiCompatible(provider) => {
                if !matches!(
                    provider.credential_status.as_str(),
                    "not-configured" | "configured"
                ) {
                    return Err(format!("Invalid credential status: {provider_id}"));
                }
                if provider.endpoint.len() > 2_048
                    || provider.model.chars().count() > 160
                    || provider.model.chars().any(char::is_control)
                {
                    return Err(format!(
                        "Provider endpoint or model is too long: {provider_id}"
                    ));
                }
                if provider.enabled
                    && (provider.endpoint.trim().is_empty() || provider.model.trim().is_empty())
                {
                    return Err(format!(
                        "Enabled provider requires endpoint and model: {provider_id}"
                    ));
                }
                if provider.endpoint.trim().is_empty() {
                    continue;
                }
                let endpoint = url::Url::parse(&provider.endpoint)
                    .map_err(|_| format!("Invalid provider endpoint: {provider_id}"))?;
                if !endpoint.username().is_empty() || endpoint.password().is_some() {
                    return Err(format!(
                        "Provider credentials must not be embedded in the endpoint: {provider_id}"
                    ));
                }
                if !matches!(endpoint.scheme(), "http" | "https") {
                    return Err(format!(
                        "Provider endpoint must use HTTP or HTTPS: {provider_id}"
                    ));
                }
                if provider.location == "local" {
                    if endpoint.scheme() != "http"
                        || !match endpoint.host() {
                            Some(url::Host::Domain(host)) => host == "localhost",
                            Some(url::Host::Ipv4(address)) => {
                                address.is_loopback() || address.is_private()
                            }
                            Some(url::Host::Ipv6(address)) => address.is_loopback(),
                            None => false,
                        }
                    {
                        return Err(format!(
                            "Local provider must use an HTTP loopback or private-network endpoint: {provider_id}"
                        ));
                    }
                } else if endpoint.scheme() != "https" {
                    return Err(format!("Cloud provider must use HTTPS: {provider_id}"));
                }
            }
            ModelProviderSettings::Larm(provider) => {
                if provider.enabled {
                    enabled_larm_count += 1;
                }
                if provider.location != "local"
                    || provider.base_url.len() > 2_048
                    || provider.token_env != "LARM_API_TOKEN"
                    || !(60..=3_600).contains(&provider.allocation_ttl_seconds)
                    || !(1..=300).contains(&provider.allocation_startup_timeout_seconds)
                    || provider.allow_fallback_by_default
                    || provider.deployment_policy != "existing-only"
                {
                    return Err(format!(
                        "LARM provider violates the fixed security policy: {provider_id}"
                    ));
                }
                let base_url = url::Url::parse(&provider.base_url)
                    .map_err(|_| format!("Invalid LARM base URL: {provider_id}"))?;
                let numeric_loopback = matches!(
                    base_url.host(),
                    Some(url::Host::Ipv4(address))
                        if address == std::net::Ipv4Addr::LOCALHOST
                ) || matches!(
                    base_url.host(),
                    Some(url::Host::Ipv6(address))
                        if address == std::net::Ipv6Addr::LOCALHOST
                );
                if base_url.scheme() != "http"
                    || !numeric_loopback
                    || base_url.port().is_none()
                    || !base_url.username().is_empty()
                    || base_url.password().is_some()
                    || base_url.query().is_some()
                    || base_url.fragment().is_some()
                    || base_url.path() != "/"
                {
                    return Err(format!(
                        "LARM base URL must be an explicit numeric HTTP loopback origin: {provider_id}"
                    ));
                }
            }
            ModelProviderSettings::Gnosis(provider) => {
                if provider.enabled {
                    enabled_gnosis_count += 1;
                }
                if provider.location != "local"
                    || providers::gnosis::control_base_url(&provider.host).is_err()
                {
                    return Err(format!(
                        "gnosis provider requires only a private-network host: {provider_id}"
                    ));
                }
            }
        }
    }
    if enabled_larm_count > 1 {
        return Err("Only one LARM provider may be enabled".to_string());
    }
    if enabled_gnosis_count > 1 {
        return Err("Only one gnosis provider may be enabled".to_string());
    }
    Ok(())
}

pub(crate) fn validate_codex_settings(settings: &CodexAgentRuntimeSettings) -> Result<(), String> {
    if settings.provider != "codex-sdk"
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
    let coding = &settings.coding_assist;
    if conversation.primary_provider_id.is_empty()
        || conversation.primary_provider_id.len() > 80
        || !conversation
            .primary_provider_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        || conversation.fallback_provider_ids.len() > 20
        || conversation
            .fallback_provider_ids
            .iter()
            .any(|provider_id| {
                provider_id.is_empty()
                    || provider_id.len() > 80
                    || !provider_id.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
                    })
            })
        || !(1_000..=300_000).contains(&conversation.timeout_ms)
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
        || settings.output_device_id.trim().is_empty()
        || settings.input_device_id.len() > 300
        || settings.output_device_id.len() > 300
        || settings.capture_mode != "push-to-talk"
        || settings.stt_provider_id != voice::gnosis_asr::PROVIDER_ID
        || settings.stt_model != voice::gnosis_asr::MODEL_ID
        || settings.tts_provider_id != "system-tts"
        || settings.tts_voice.trim().is_empty()
        || settings.tts_voice.chars().count() > 160
        || settings.cloud_fallback_enabled
    {
        return Err("Invalid local voice settings".to_string());
    }
    Ok(())
}

pub(crate) fn validate_security_settings(settings: &SecurityRuntimeSettings) -> Result<(), String> {
    if settings.credential_storage != "environment" || !settings.diagnostics_redaction {
        return Err(
            "Secrets must remain outside SQLite and diagnostics must be redacted".to_string(),
        );
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
        default_settings_input, direct_provider, gnosis_provider, larm_provider, provider,
    };
    use crate::{
        initialize_database, providers, voice, CodexAgentRuntimeSettings, ModelProviderSettings,
        ModelProvidersSettings, OpenAiCompatibleProviderSettings, GNOSIS_PROVIDER_ID,
    };
    use rusqlite::Connection;
    use serde_json::{json, Value};

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
        })
        .is_err());
        let mut gnosis = direct_provider(GNOSIS_PROVIDER_ID, "local");
        gnosis.endpoint = "http://192.168.0.65:8083/v1".to_string();
        gnosis.model = "ornith15-35b".to_string();
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(gnosis)],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
        })
        .is_ok());
        let mut public_http = direct_provider("public-http", "local");
        public_http.endpoint = "http://203.0.113.10:8080/v1".to_string();
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(public_http)],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
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
        };
        assert!(validate_model_providers(&unsafe_id).is_err());
        let ambiguous_ids = ModelProvidersSettings {
            providers: vec![provider("local-a", "local"), provider("local_a", "local")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
        };
        assert!(validate_model_providers(&ambiguous_ids).is_err());

        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings")
            .value_json = json!({
            "providers": [provider("local", "local"), provider("cloud", "cloud")],
            "reasoningEffort": "medium"
        });
        let routing = documents
            .iter_mut()
            .find(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        routing.value_json["conversationRespond"]["primaryProviderId"] = json!("local");
        routing.value_json["conversationRespond"]["fallbackProviderIds"] = json!(["cloud"]);
        assert!(validate_settings_batch(&documents).is_err());

        let mut documents = default_settings_input();
        documents
            .iter_mut()
            .find(|document| document.namespace == "providers.model")
            .expect("provider settings")
            .value_json = json!({
            "providers": [
                gnosis_provider("gnosis-primary"),
                provider("local-fallback", "local")
            ],
            "reasoningEffort": "medium"
        });
        let routing_index = documents
            .iter()
            .position(|document| document.namespace == "routing.tasks")
            .expect("routing settings");
        documents[routing_index].value_json["conversationRespond"]["primaryProviderId"] =
            json!("gnosis-primary");
        documents[routing_index].value_json["conversationRespond"]["fallbackProviderIds"] =
            json!(["local-fallback"]);
        assert!(validate_settings_batch(&documents).is_err());

        documents[routing_index].value_json["conversationRespond"]["fallbackProviderIds"] =
            json!([]);
        documents[routing_index].value_json["conversationRespond"]["timeoutMs"] =
            json!(providers::gnosis::MAX_REQUEST_TIMEOUT_MS + 1);
        assert!(validate_settings_batch(&documents).is_err());
        documents[routing_index].value_json["conversationRespond"]["timeoutMs"] =
            json!(providers::gnosis::MAX_REQUEST_TIMEOUT_MS);
        assert!(validate_settings_batch(&documents).is_ok());
    }

    #[test]
    fn larm_settings_enforce_the_fixed_loopback_security_contract() {
        let valid = ModelProvidersSettings {
            providers: vec![larm_provider("larm")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
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
                })
                .is_err(),
                "invalid LARM URL was accepted: {base_url}"
            );
        }

        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![larm_provider("larm-a"), larm_provider("larm-b")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
        })
        .is_err());
    }

    #[test]
    fn legacy_provider_ids_and_default_codex_model_remain_valid() {
        let providers = ModelProvidersSettings {
            providers: vec![provider("Local_Custom", "local")],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
        };
        assert!(validate_model_providers(&providers).is_ok());

        let codex = CodexAgentRuntimeSettings {
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
        })
        .is_ok());

        provider.label.push('あ');
        assert!(validate_model_providers(&ModelProvidersSettings {
            providers: vec![ModelProviderSettings::OpenAiCompatible(provider)],
            reasoning_effort: providers::default_conversation_reasoning_effort(),
        })
        .is_err());

        let mut documents = default_settings_input();
        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["ttsVoice"] = json!("声".repeat(160));
        validate_settings_document(voice).expect("localized TTS voice is accepted");
        voice.value_json["ttsVoice"] = json!("声".repeat(161));
        assert!(validate_settings_document(voice).is_err());
    }

    #[test]
    fn voice_settings_require_the_fixed_gnosis_asr_contract() {
        let mut documents = default_settings_input();
        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["sttProviderId"] = json!("local-whisper");
        assert_eq!(
            validate_settings_document(voice).expect_err("local Whisper is rejected"),
            "Invalid local voice settings"
        );

        let voice = documents
            .iter_mut()
            .find(|document| document.namespace == "voice.runtime")
            .expect("voice settings");
        voice.value_json["sttProviderId"] = json!(voice::gnosis_asr::PROVIDER_ID);
        voice.value_json["sttModel"] = json!(voice::gnosis_asr::MODEL_ID);
        validate_settings_document(voice).expect("gnosis ASR contract is accepted");
    }
}
