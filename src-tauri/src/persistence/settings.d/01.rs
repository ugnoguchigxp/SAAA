pub(crate) const SETTINGS_SCHEMA_VERSION: i64 = 15;
const DEFAULT_CONVERSATION_TIMEOUT_MS: u64 = 1_800_000;
const MAX_CONVERSATION_TIMEOUT_MS: u64 = 3_600_000;
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
    state
        .sqlite_writer
        .write(|connection| set_voice_listening_enabled_to_connection(connection, enabled))
}
pub(crate) fn load_routing_settings(connection: &Connection) -> Result<RoutingSettings, String> {
    let document = read_settings_document(connection, "routing.tasks", "default")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode route settings: {error}"))?;
    validate_routing_settings(&settings)?;
    Ok(settings)
}
pub(crate) fn load_role_routing_settings(
    connection: &Connection,
) -> Result<crate::role_routing::RoleRoutingSettings, String> {
    let document = read_settings_document(connection, "routing.roles", "default")?;
    let settings = serde_json::from_value(document.value_json)
        .map_err(|error| format!("Could not decode role routing settings: {error}"))?;
    crate::role_routing::contracts::validate_settings(&settings)?;
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
    let role_routing_was_enabled = load_role_routing_settings(connection)
        .map(|settings| settings.enabled)
        .unwrap_or(false);
    let role_routing_will_be_enabled = documents
        .iter()
        .find(|document| document.namespace == "routing.roles" && document.key == "default")
        .and_then(|document| document.value_json.get("enabled"))
        .and_then(serde_json::Value::as_bool);
    let disable_role_routing =
        role_routing_was_enabled && role_routing_will_be_enabled == Some(false);
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
    if documents
        .iter()
        .any(|document| document.namespace == "routing.roles" && document.key == "default")
    {
        crate::role_routing::repository::capture_current_policy(
            &transaction,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0),
        )?;
    }
    if disable_role_routing {
        crate::role_routing::repository::cancel_all_for_disable(
            &transaction,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0),
        )?;
    }
    transaction.commit().map_err(database_error)?;

    let saved = documents
        .iter()
        .map(|document| read_settings_document(connection, &document.namespace, &document.key))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(saved)
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
            | ("routing.roles", "default")
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
        ("routing.roles", "default") => {
            let settings = serde_json::from_value::<crate::role_routing::RoleRoutingSettings>(
                input.value_json.clone(),
            )
            .map_err(|error| format!("Invalid role routing settings: {error}"))?;
            crate::role_routing::contracts::validate_settings(&settings)
        }
        _ => Err("Unsupported settings document".to_string()),
    }
}
pub(crate) fn validate_settings_batch(
    documents: &[SaveSettingsDocumentInput],
) -> Result<(), String> {
    if !(7..=8).contains(&documents.len()) {
        return Err("A complete settings snapshot is required".to_string());
    }
    let unique = documents
        .iter()
        .map(|document| (document.namespace.as_str(), document.key.as_str()))
        .collect::<std::collections::HashSet<_>>();
    if unique.len() != documents.len() {
        return Err("Each settings document must appear exactly once".to_string());
    }
    if documents.len() == 8
        && !documents
            .iter()
            .any(|document| document.namespace == "routing.roles" && document.key == "default")
    {
        return Err("Role routing settings are required in an eight-document snapshot".to_string());
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
    let role_policy = documents
        .iter()
        .find(|document| document.namespace == "routing.roles" && document.key == "default");
    let codex = documents
        .iter()
        .find(|document| document.namespace == "providers.agent" && document.key == "codex-sdk")
        .ok_or_else(|| "Codex settings are required".to_string())?;
    let providers = serde_json::from_value::<ModelProvidersSettings>(providers.value_json.clone())
        .map_err(|error| format!("Invalid model provider settings: {error}"))?;
    let routing = serde_json::from_value::<RoutingSettings>(routing.value_json.clone())
        .map_err(|error| format!("Invalid routing settings: {error}"))?;
    let security = serde_json::from_value::<SecurityRuntimeSettings>(security.value_json.clone())
        .map_err(|error| format!("Invalid security settings: {error}"))?;
    let codex = serde_json::from_value::<CodexAgentRuntimeSettings>(codex.value_json.clone())
        .map_err(|error| format!("Invalid Codex settings: {error}"))?;
    if let Some(role_policy) = role_policy {
        let policy = serde_json::from_value::<crate::role_routing::RoleRoutingSettings>(
            role_policy.value_json.clone(),
        )
        .map_err(|error| format!("Invalid role routing settings: {error}"))?;
        validate_role_routing_provider_bindings(&policy, &providers, &codex)?;
    }
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
    if conversation.source == "provider" && primary_id.is_some() {
        let primary = primary_id
            .and_then(enabled_provider)
            .ok_or_else(|| "The individual conversation provider must be enabled".to_string())?;
        if !matches!(
            primary,
            ModelProviderSettings::OpenAiCompatible(_)
                | ModelProviderSettings::AgentSession(_)
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
    if conversation.source == "provider"
        && primary_id.is_none()
        && !conversation.fallback_provider_ids.is_empty()
    {
        return Err("A fallback requires a primary conversation provider".to_string());
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
                | ModelProviderSettings::AgentSession(_)
                | ModelProviderSettings::DynamicLan(_)
        ) {
            return Err(format!(
                "Fallback provider does not support LLM: {provider_id}"
            ));
        }
        if !route_ids.insert(provider_id) {
            return Err(format!("Duplicate provider in route: {provider_id}"));
        }
        let primary_is_local = conversation.source == "harness"
            || primary_id.is_some_and(|primary_id| {
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

    voice_fallbacks::validate(&providers, &routing, &security)?;
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
fn validate_role_routing_provider_bindings(
    policy: &crate::role_routing::RoleRoutingSettings,
    providers: &ModelProvidersSettings,
    codex: &CodexAgentRuntimeSettings,
) -> Result<(), String> {
    if !policy.enabled {
        return Ok(());
    }
    for actor in &policy.actors {
        match actor.transport.as_str() {
            "provider" => {
                let provider = providers
                    .providers
                    .iter()
                    .find(|provider| {
                        actor.provider_id.as_deref() == Some(provider.id()) && provider.enabled()
                    })
                    .ok_or_else(|| {
                        format!("Role routing actor provider is not enabled: {}", actor.id)
                    })?;
                if !matches!(
                    provider,
                    ModelProviderSettings::OpenAiCompatible(_)
                        | ModelProviderSettings::AgentSession(_)
                        | ModelProviderSettings::DynamicLan(_)
                ) {
                    return Err(format!(
                        "Role routing actor provider does not support LLM: {}",
                        actor.id
                    ));
                }
                if provider.location() != actor.location {
                    return Err(format!(
                        "Role routing actor location conflicts with provider: {}",
                        actor.id
                    ));
                }
            }
            "codex_sdk" if codex.enabled && codex.health == "ready" => {}
            "codex_sdk" => {
                return Err(format!(
                    "Role routing Codex actor is unavailable until the Codex SDK is enabled and ready: {}",
                    actor.id
                ));
            }
            _ => return Err("Invalid role routing actor transport".to_string()),
        }
    }
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
