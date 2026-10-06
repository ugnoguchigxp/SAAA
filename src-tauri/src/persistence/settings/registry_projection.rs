//! Voice bindings are shared with the legacy `routing.tasks` voice routes so the
//! unchanged voice pipeline and the registry always agree on one value.
use super::documents::validate_voice_route_provider;
use super::{
    database_error, load_model_providers, load_routing_settings, now_iso,
    validate_routing_settings, voice_fallbacks,
};
use crate::providers::service_registry::{Purpose, PurposeBinding, RegistrySnapshot};
use crate::{ModelProviderSettings, VoiceRouteSettings};
use rusqlite::{params, Connection};

/// A voice binding that uses a service created through the registry (as primary or as a
/// fallback) cannot be expressed by the legacy routes, so the registry owns it.
pub(crate) fn registry_owned(binding: &PurposeBinding) -> bool {
    binding
        .primary_resource_id
        .iter()
        .chain(binding.fallback_resource_ids.iter())
        .any(|id| id.starts_with("res:svc-"))
}

fn legacy_provider_id<'a>(
    resource_id: &'a str,
    harness_resource: &str,
) -> Result<Option<&'a str>, String> {
    if resource_id == harness_resource {
        return Ok(None);
    }
    resource_id
        .strip_prefix("res:")
        .filter(|id| !id.starts_with("svc-"))
        .map(Some)
        .ok_or_else(|| format!("Voice route cannot use this resource yet: {resource_id}"))
}

fn route_from_binding(
    binding: &PurposeBinding,
    harness_resource: &str,
) -> Result<VoiceRouteSettings, String> {
    let primary = binding
        .primary_resource_id
        .as_deref()
        .filter(|_| binding.enabled)
        .ok_or("A voice purpose cannot be left unselected")?;
    let provider_id = legacy_provider_id(primary, harness_resource)?;
    let mut fallbacks = Vec::new();
    for id in &binding.fallback_resource_ids {
        fallbacks.push(
            legacy_provider_id(id, harness_resource)?
                .ok_or("The LARM service cannot be a voice fallback")?
                .to_string(),
        );
    }
    Ok(VoiceRouteSettings {
        fallback_provider_ids: fallbacks,
        attempt_timeout_ms: binding.attempt_timeout_ms,
        source: if provider_id.is_some() {
            "provider"
        } else {
            "harness"
        }
        .to_string(),
        provider_id: provider_id.map(str::to_string),
        timeout_ms: binding.timeout_ms,
    })
}

/// Writes the registry's voice bindings into `routing.tasks` after the same
/// validation the legacy settings save applies.
pub(crate) fn project_voice_bindings(
    connection: &Connection,
    snapshot: &RegistrySnapshot,
) -> Result<(), String> {
    let providers = load_model_providers(connection)?;
    let mut routing = load_routing_settings(connection)?;
    for (purpose, harness) in [
        (Purpose::VoiceTranscribe, "res:harness-asr"),
        (Purpose::VoiceSpeak, "res:harness-tts"),
    ] {
        let binding = snapshot
            .binding(purpose)
            .ok_or("Voice binding is missing")?;
        if registry_owned(binding) {
            if !binding.enabled {
                return Err("登録した音声サービスを使う用途は無効化できません".into());
            }
            // The purpose registry is authoritative for new resources. Legacy
            // settings cannot losslessly express their credential references.
            continue;
        }
        let route = route_from_binding(binding, harness)?;
        match purpose {
            Purpose::VoiceTranscribe => routing.voice_transcribe = route,
            _ => routing.voice_speak = route,
        }
    }
    validate_routing_settings(&routing)?;
    voice_fallbacks::validate(&providers, &routing)?;
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
    let value = serde_json::to_string(&routing)
        .map_err(|error| format!("Could not encode route settings: {error}"))?;
    connection
        .execute(
            "UPDATE settings_documents SET value_json=?1, updated_at=?2
             WHERE namespace='routing.tasks' AND key='default'",
            params![value, now_iso()],
        )
        .map_err(database_error)?;
    Ok(())
}
