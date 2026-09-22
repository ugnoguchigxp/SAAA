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
    for (total, attempt) in [
        (conversation.timeout_ms, conversation.attempt_timeout_ms),
        (transcribe.timeout_ms, transcribe.attempt_timeout_ms),
        (speak.timeout_ms, speak.attempt_timeout_ms),
    ] {
        if attempt.is_some_and(|n| n < 1_000 || n > total) {
            return Err("Invalid attempt timeout".into());
        }
    }
    for route in [transcribe, speak] {
        if route.fallback_provider_ids.len() > 20
            || route
                .fallback_provider_ids
                .iter()
                .any(|id| !valid_provider_id(id))
        {
            return Err("Invalid voice fallbacks".into());
        }
    }
    // An unconfigured conversation route is readable after provider removal.
    // A missing primary must remain unconfigured rather than selecting a fallback.
    let conversation_source_valid = valid_source(
        &conversation.source,
        conversation.primary_provider_id.as_deref(),
    ) || (conversation.source == "provider"
        && conversation.primary_provider_id.is_none());
    if !conversation_source_valid
        || conversation.fallback_provider_ids.len() > 20
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
        .prepare_cached(
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
