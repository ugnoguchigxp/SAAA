use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub(crate) enum Purpose {
    #[serde(rename = "conversation.respond")]
    ConversationRespond,
    #[serde(rename = "voice.transcribe")]
    VoiceTranscribe,
    #[serde(rename = "voice.speak")]
    VoiceSpeak,
    #[serde(rename = "media.image.generate")]
    MediaImageGenerate,
    #[serde(rename = "media.music.generate")]
    MediaMusicGenerate,
}

impl Purpose {
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::ConversationRespond => "conversation.respond",
            Self::VoiceTranscribe => "voice.transcribe",
            Self::VoiceSpeak => "voice.speak",
            Self::MediaImageGenerate => "media.image.generate",
            Self::MediaMusicGenerate => "media.music.generate",
        }
    }

    pub(crate) fn required_capability(self) -> Capability {
        match self {
            Self::ConversationRespond => Capability::TextGeneration,
            Self::VoiceTranscribe => Capability::Transcription,
            Self::VoiceSpeak => Capability::Speech,
            Self::MediaImageGenerate => Capability::ImageGeneration,
            Self::MediaMusicGenerate => Capability::MusicGeneration,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Capability {
    TextGeneration,
    Transcription,
    Speech,
    ImageGeneration,
    MusicGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AdapterKind {
    Larm,
    ChatCompletions,
    AnthropicMessages,
    ReplicateMedia,
    AgentSession,
    HttpAsr,
    HttpTts,
    SystemTts,
}

/// Opaque reference to a named secret. Never holds the secret body.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CredentialRef {
    pub(crate) service: String,
    pub(crate) account: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ServiceConnection {
    pub(crate) connection_id: String,
    pub(crate) label: String,
    pub(crate) adapter_kind: AdapterKind,
    /// Endpoint, base URL or Harness address. Empty for adapters without one.
    pub(crate) endpoint: String,
    /// "local" or "cloud", as stored by the legacy provider.
    pub(crate) location: String,
    pub(crate) authentication: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) credential_ref: Option<CredentialRef>,
    pub(crate) enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ServiceResource {
    pub(crate) resource_id: String,
    pub(crate) connection_id: String,
    pub(crate) capability: Capability,
    /// Model selector; empty when the connection selects it (for example LARM).
    pub(crate) model: String,
    /// Voice, language or other per-purpose setting preserved from legacy settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    /// Adapter options are validated at save time and pinned with the resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request_options: Option<serde_json::Value>,
    pub(crate) enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BindingReview {
    Ready,
    /// Stored selection and the path actually executed disagree. The user must
    /// apply the binding explicitly before it is used.
    NeedsReview,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PurposeBinding {
    pub(crate) purpose: Purpose,
    pub(crate) enabled: bool,
    pub(crate) primary_resource_id: Option<String>,
    #[serde(default)]
    pub(crate) fallback_resource_ids: Vec<String>,
    /// Explicit per-purpose cloud permission. Absent in older snapshots means
    /// the existing, deliberately selected cloud configuration is preserved.
    #[serde(default = "cloud_allowed_default")]
    pub(crate) cloud_allowed: bool,
    pub(crate) timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attempt_timeout_ms: Option<u64>,
    /// Legacy selection preserved while `review` is `NeedsReview`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stored_primary_resource_id: Option<String>,
    pub(crate) review: BindingReview,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RegistrySnapshot {
    pub(crate) connections: Vec<ServiceConnection>,
    pub(crate) resources: Vec<ServiceResource>,
    pub(crate) bindings: Vec<PurposeBinding>,
}

impl RegistrySnapshot {
    pub(crate) fn connection(&self, id: &str) -> Option<&ServiceConnection> {
        self.connections.iter().find(|c| c.connection_id == id)
    }

    pub(crate) fn resource(&self, id: &str) -> Option<&ServiceResource> {
        self.resources.iter().find(|r| r.resource_id == id)
    }

    pub(crate) fn binding(&self, purpose: Purpose) -> Option<&PurposeBinding> {
        self.bindings.iter().find(|b| b.purpose == purpose)
    }
}

#[cfg(test)]
impl RegistrySnapshot {
    pub(crate) fn resource_mut_for_test(&mut self, id: &str) -> &mut ServiceResource {
        self.resources
            .iter_mut()
            .find(|r| r.resource_id == id)
            .expect("test resource")
    }
}

fn cloud_allowed_default() -> bool {
    true
}
