use super::types::*;
use crate::credentials::PROVIDER_CREDENTIAL_SERVICE;
use crate::{ModelProviderSettings, ModelProvidersSettings, RoutingSettings};

pub(super) const HARNESS_CONNECTION_ID: &str = "conn:harness";

pub(crate) fn connection_id(legacy_provider_id: &str) -> String {
    format!("conn:{legacy_provider_id}")
}

pub(crate) fn resource_id(legacy_provider_id: &str) -> String {
    format!("res:{legacy_provider_id}")
}

fn harness_resource_id(capability: Capability) -> String {
    let suffix = match capability {
        Capability::TextGeneration => "llm",
        Capability::Transcription => "asr",
        Capability::Speech => "tts",
    };
    format!("res:harness-{suffix}")
}

fn credential_ref(authentication: &str, provider_id: &str) -> Option<CredentialRef> {
    (authentication == "api-key").then(|| CredentialRef {
        service: PROVIDER_CREDENTIAL_SERVICE.to_string(),
        account: provider_id.to_string(),
    })
}

/// Deterministic, lossless conversion of legacy provider and route settings.
/// Each legacy provider keeps its own connection and secret reference; nothing
/// is merged because endpoints match. Running it twice yields the same value.
pub(crate) fn migrate_legacy(
    providers: &ModelProvidersSettings,
    routing: &RoutingSettings,
) -> Result<RegistrySnapshot, String> {
    let mut snapshot = RegistrySnapshot::default();
    snapshot.connections.push(ServiceConnection {
        connection_id: HARNESS_CONNECTION_ID.to_string(),
        label: "LARM".to_string(),
        adapter_kind: AdapterKind::Larm,
        endpoint: providers.harness.address.clone(),
        location: "local".to_string(),
        authentication: "none".to_string(),
        credential_ref: None,
        enabled: true,
    });
    for capability in [
        Capability::TextGeneration,
        Capability::Transcription,
        Capability::Speech,
    ] {
        snapshot.resources.push(ServiceResource {
            resource_id: harness_resource_id(capability),
            connection_id: HARNESS_CONNECTION_ID.to_string(),
            capability,
            model: String::new(),
            detail: providers
                .harness
                .tts_voice
                .clone()
                .filter(|_| capability == Capability::Speech),
            enabled: true,
        });
    }
    for provider in &providers.providers {
        let (adapter_kind, capability, endpoint, authentication, model, detail) = match provider {
            ModelProviderSettings::OpenAiCompatible(p) => (
                AdapterKind::ChatCompletions,
                Capability::TextGeneration,
                p.endpoint.clone(),
                p.authentication.clone(),
                p.model.clone(),
                None,
            ),
            ModelProviderSettings::AgentSession(p) => (
                AdapterKind::AgentSession,
                Capability::TextGeneration,
                p.base_url.clone(),
                p.authentication.clone(),
                p.model.clone(),
                None,
            ),
            ModelProviderSettings::DynamicLan(p) => (
                AdapterKind::Larm,
                Capability::TextGeneration,
                p.host.clone(),
                "none".to_string(),
                String::new(),
                None,
            ),
            ModelProviderSettings::CloudAsr(p) => (
                AdapterKind::HttpAsr,
                Capability::Transcription,
                p.endpoint.clone(),
                p.authentication.clone(),
                p.model.clone(),
                Some(p.language.clone()),
            ),
            ModelProviderSettings::CloudTts(p) => (
                AdapterKind::HttpTts,
                Capability::Speech,
                p.endpoint.clone(),
                p.authentication.clone(),
                p.model.clone(),
                Some(p.voice.clone()),
            ),
            ModelProviderSettings::SystemTts(p) => (
                AdapterKind::SystemTts,
                Capability::Speech,
                String::new(),
                "none".to_string(),
                String::new(),
                Some(p.voice.clone()),
            ),
        };
        snapshot.connections.push(ServiceConnection {
            connection_id: connection_id(provider.id()),
            label: provider.label().to_string(),
            adapter_kind,
            endpoint,
            location: provider.location().to_string(),
            credential_ref: credential_ref(&authentication, provider.id()),
            authentication,
            enabled: provider.enabled(),
        });
        snapshot.resources.push(ServiceResource {
            resource_id: resource_id(provider.id()),
            connection_id: connection_id(provider.id()),
            capability,
            model,
            detail,
            enabled: provider.enabled(),
        });
    }

    let known = |id: &str| providers.providers.iter().any(|p| p.id() == id);
    let provider_resource = |id: &str| -> Result<String, String> {
        known(id)
            .then(|| resource_id(id))
            .ok_or_else(|| format!("Legacy route references an unknown provider: {id}"))
    };
    let fallbacks = |ids: &[String]| -> Result<Vec<String>, String> {
        ids.iter().map(|id| provider_resource(id)).collect()
    };

    let conversation = &routing.conversation_respond;
    let stored_primary = conversation
        .primary_provider_id
        .as_deref()
        .map(provider_resource)
        .transpose()?;
    // The current queue always runs the conversation on LARM, so a stored
    // provider selection disagrees with the executed path and needs review.
    let conversation_from_provider = conversation.source == "provider";
    snapshot.bindings.push(PurposeBinding {
        purpose: Purpose::ConversationRespond,
        enabled: true,
        primary_resource_id: Some(harness_resource_id(Capability::TextGeneration))
            .filter(|_| conversation_from_provider || conversation.source == "harness"),
        fallback_resource_ids: if conversation_from_provider {
            fallbacks(&conversation.fallback_provider_ids)?
        } else {
            Vec::new()
        },
        timeout_ms: conversation.timeout_ms,
        attempt_timeout_ms: conversation.attempt_timeout_ms,
        stored_primary_resource_id: stored_primary.filter(|_| conversation_from_provider),
        review: if conversation_from_provider {
            BindingReview::NeedsReview
        } else {
            BindingReview::Ready
        },
    });

    for (purpose, route) in [
        (Purpose::VoiceTranscribe, &routing.voice_transcribe),
        (Purpose::VoiceSpeak, &routing.voice_speak),
    ] {
        let primary = if route.source == "harness" {
            Some(harness_resource_id(purpose.required_capability()))
        } else {
            route
                .provider_id
                .as_deref()
                .map(provider_resource)
                .transpose()?
        };
        snapshot.bindings.push(PurposeBinding {
            purpose,
            enabled: primary.is_some(),
            primary_resource_id: primary,
            fallback_resource_ids: fallbacks(&route.fallback_provider_ids)?,
            timeout_ms: route.timeout_ms,
            attempt_timeout_ms: route.attempt_timeout_ms,
            stored_primary_resource_id: None,
            review: BindingReview::Ready,
        });
    }
    super::validate_snapshot(&snapshot)?;
    Ok(snapshot)
}
