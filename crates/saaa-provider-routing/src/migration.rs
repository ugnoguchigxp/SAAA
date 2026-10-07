use super::types::*;
use crate::PROVIDER_CREDENTIAL_SERVICE;

pub const HARNESS_CONNECTION_ID: &str = "conn:harness";

#[derive(Debug, Clone)]
pub struct LegacyHarness {
    pub address: String,
    pub tts_voice: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LegacyProvider {
    pub id: String,
    pub label: String,
    pub location: String,
    pub enabled: bool,
    pub adapter_kind: AdapterKind,
    pub capability: Capability,
    pub endpoint: String,
    pub authentication: String,
    pub model: String,
    pub detail: Option<String>,
    pub request_options: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct LegacyRoute {
    pub source: String,
    pub primary_provider_id: Option<String>,
    pub provider_id: Option<String>,
    pub fallback_provider_ids: Vec<String>,
    pub timeout_ms: u64,
    pub attempt_timeout_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct LegacySettings {
    pub harness: LegacyHarness,
    pub providers: Vec<LegacyProvider>,
    pub conversation_respond: LegacyRoute,
    pub voice_transcribe: LegacyRoute,
    pub voice_speak: LegacyRoute,
}

pub fn connection_id(legacy_provider_id: &str) -> String {
    format!("conn:{legacy_provider_id}")
}

pub fn resource_id(legacy_provider_id: &str) -> String {
    format!("res:{legacy_provider_id}")
}

fn harness_resource_id(capability: Capability) -> String {
    let suffix = match capability {
        Capability::TextGeneration => "llm",
        Capability::Transcription => "asr",
        Capability::Speech => "tts",
        Capability::ImageGeneration => "image",
        Capability::MusicGeneration => "music",
    };
    format!("res:harness-{suffix}")
}

fn credential_ref(authentication: &str, provider_id: &str) -> Option<CredentialRef> {
    (authentication == "api-key").then(|| CredentialRef {
        service: PROVIDER_CREDENTIAL_SERVICE.to_string(),
        account: provider_id.to_string(),
    })
}

/// Deterministic conversion of legacy provider and route settings.
/// Each legacy provider keeps its own connection and secret reference.
pub fn migrate_legacy(settings: &LegacySettings) -> Result<RegistrySnapshot, String> {
    let mut snapshot = RegistrySnapshot::default();
    snapshot.connections.push(ServiceConnection {
        connection_id: HARNESS_CONNECTION_ID.to_string(),
        label: "LARM".to_string(),
        adapter_kind: AdapterKind::Larm,
        endpoint: settings.harness.address.clone(),
        location: "local".to_string(),
        authentication: "none".to_string(),
        credential_ref: None,
        enabled: true,
    });
    for capability in [
        Capability::TextGeneration,
        Capability::Transcription,
        Capability::Speech,
        Capability::ImageGeneration,
        Capability::MusicGeneration,
    ] {
        snapshot.resources.push(ServiceResource {
            resource_id: harness_resource_id(capability),
            connection_id: HARNESS_CONNECTION_ID.to_string(),
            capability,
            model: String::new(),
            request_options: None,
            detail: settings
                .harness
                .tts_voice
                .clone()
                .filter(|_| capability == Capability::Speech),
            enabled: true,
        });
    }
    for provider in &settings.providers {
        snapshot.connections.push(ServiceConnection {
            connection_id: connection_id(&provider.id),
            label: provider.label.clone(),
            adapter_kind: provider.adapter_kind,
            endpoint: provider.endpoint.clone(),
            location: provider.location.clone(),
            credential_ref: credential_ref(&provider.authentication, &provider.id),
            authentication: provider.authentication.clone(),
            enabled: provider.enabled,
        });
        snapshot.resources.push(ServiceResource {
            resource_id: resource_id(&provider.id),
            connection_id: connection_id(&provider.id),
            capability: provider.capability,
            model: provider.model.clone(),
            detail: provider.detail.clone(),
            request_options: provider.request_options.clone(),
            enabled: provider.enabled,
        });
    }

    let known = |id: &str| settings.providers.iter().any(|provider| provider.id == id);
    let provider_resource = |id: &str| -> Result<String, String> {
        known(id)
            .then(|| resource_id(id))
            .ok_or_else(|| format!("Legacy route references an unknown provider: {id}"))
    };
    let fallbacks = |ids: &[String]| -> Result<Vec<String>, String> {
        ids.iter().map(|id| provider_resource(id)).collect()
    };

    let conversation = &settings.conversation_respond;
    let stored_primary = conversation
        .primary_provider_id
        .as_deref()
        .map(provider_resource)
        .transpose()?;
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
        cloud_allowed: true,
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
        (Purpose::VoiceTranscribe, &settings.voice_transcribe),
        (Purpose::VoiceSpeak, &settings.voice_speak),
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
            cloud_allowed: true,
            timeout_ms: route.timeout_ms,
            attempt_timeout_ms: route.attempt_timeout_ms,
            stored_primary_resource_id: None,
            review: BindingReview::Ready,
        });
    }
    for (purpose, capability) in [
        (Purpose::MediaImageGenerate, Capability::ImageGeneration),
        (Purpose::MediaMusicGenerate, Capability::MusicGeneration),
    ] {
        snapshot.bindings.push(PurposeBinding {
            purpose,
            enabled: true,
            primary_resource_id: Some(harness_resource_id(capability)),
            fallback_resource_ids: Vec::new(),
            timeout_ms: 1_800_000,
            attempt_timeout_ms: Some(120_000),
            cloud_allowed: false,
            stored_primary_resource_id: None,
            review: BindingReview::Ready,
        });
    }
    crate::validate_snapshot(&snapshot)?;
    Ok(snapshot)
}
