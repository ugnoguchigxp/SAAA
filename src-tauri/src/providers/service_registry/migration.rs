use crate::{ModelProviderSettings, ModelProvidersSettings, RoutingSettings};
use saaa_provider_routing::{LegacyHarness, LegacyProvider, LegacyRoute, LegacySettings};

pub(crate) fn migrate_legacy(
    providers: &ModelProvidersSettings,
    routing: &RoutingSettings,
) -> Result<super::RegistrySnapshot, String> {
    saaa_provider_routing::migrate_legacy(&LegacySettings {
        harness: LegacyHarness {
            address: providers.harness.address.clone(),
            tts_voice: providers.harness.tts_voice.clone(),
        },
        providers: providers
            .providers
            .iter()
            .map(legacy_provider)
            .collect::<Result<Vec<_>, _>>()?,
        conversation_respond: legacy_conversation(&routing.conversation_respond),
        voice_transcribe: legacy_voice(&routing.voice_transcribe),
        voice_speak: legacy_voice(&routing.voice_speak),
    })
}

fn legacy_conversation(route: &crate::ConversationRouteSettings) -> LegacyRoute {
    LegacyRoute {
        source: route.source.clone(),
        primary_provider_id: route.primary_provider_id.clone(),
        provider_id: None,
        fallback_provider_ids: route.fallback_provider_ids.clone(),
        timeout_ms: route.timeout_ms,
        attempt_timeout_ms: route.attempt_timeout_ms,
    }
}

fn legacy_voice(route: &crate::VoiceRouteSettings) -> LegacyRoute {
    LegacyRoute {
        source: route.source.clone(),
        primary_provider_id: None,
        provider_id: route.provider_id.clone(),
        fallback_provider_ids: route.fallback_provider_ids.clone(),
        timeout_ms: route.timeout_ms,
        attempt_timeout_ms: route.attempt_timeout_ms,
    }
}

fn legacy_provider(provider: &ModelProviderSettings) -> Result<LegacyProvider, String> {
    let (adapter_kind, capability, endpoint, authentication, model, detail, request_options) =
        match provider {
            ModelProviderSettings::OpenAiCompatible(provider) => (
                super::AdapterKind::ChatCompletions,
                super::Capability::TextGeneration,
                provider.endpoint.clone(),
                provider.authentication.clone(),
                provider.model.clone(),
                None,
                provider
                    .request_options
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|error| error.to_string())?,
            ),
            ModelProviderSettings::AgentSession(provider) => (
                super::AdapterKind::AgentSession,
                super::Capability::TextGeneration,
                provider.base_url.clone(),
                provider.authentication.clone(),
                provider.model.clone(),
                None,
                None,
            ),
            ModelProviderSettings::DynamicLan(provider) => (
                super::AdapterKind::Larm,
                super::Capability::TextGeneration,
                provider.host.clone(),
                "none".to_string(),
                String::new(),
                None,
                None,
            ),
            ModelProviderSettings::CloudAsr(provider) => (
                super::AdapterKind::HttpAsr,
                super::Capability::Transcription,
                provider.endpoint.clone(),
                provider.authentication.clone(),
                provider.model.clone(),
                Some(provider.language.clone()),
                None,
            ),
            ModelProviderSettings::CloudTts(provider) => (
                super::AdapterKind::HttpTts,
                super::Capability::Speech,
                provider.endpoint.clone(),
                provider.authentication.clone(),
                provider.model.clone(),
                Some(provider.voice.clone()),
                None,
            ),
            ModelProviderSettings::SystemTts(provider) => (
                super::AdapterKind::SystemTts,
                super::Capability::Speech,
                String::new(),
                "none".to_string(),
                String::new(),
                Some(provider.voice.clone()),
                None,
            ),
        };
    Ok(LegacyProvider {
        id: provider.id().to_string(),
        label: provider.label().to_string(),
        location: provider.location().to_string(),
        enabled: provider.enabled(),
        adapter_kind,
        capability,
        endpoint,
        authentication,
        model,
        detail,
        request_options,
    })
}
