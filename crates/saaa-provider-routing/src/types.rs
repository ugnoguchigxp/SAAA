use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum Purpose {
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
    pub fn id(self) -> &'static str {
        match self {
            Self::ConversationRespond => "conversation.respond",
            Self::VoiceTranscribe => "voice.transcribe",
            Self::VoiceSpeak => "voice.speak",
            Self::MediaImageGenerate => "media.image.generate",
            Self::MediaMusicGenerate => "media.music.generate",
        }
    }

    pub fn required_capability(self) -> Capability {
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
pub enum Capability {
    TextGeneration,
    Transcription,
    Speech,
    ImageGeneration,
    MusicGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterKind {
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
pub struct CredentialRef {
    pub service: String,
    pub account: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceConnection {
    pub connection_id: String,
    pub label: String,
    pub adapter_kind: AdapterKind,
    /// Endpoint, base URL or Harness address. Empty for adapters without one.
    pub endpoint: String,
    /// "local" or "cloud", as stored by the legacy provider.
    pub location: String,
    pub authentication: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<CredentialRef>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceResource {
    pub resource_id: String,
    pub connection_id: String,
    pub capability: Capability,
    /// Model selector; empty when the connection selects it (for example LARM).
    pub model: String,
    /// Voice, language or other per-purpose setting preserved from legacy settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Adapter options are validated at save time and pinned with the resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_options: Option<serde_json::Value>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BindingReview {
    Ready,
    /// Stored selection and the path actually executed disagree. The user must
    /// apply the binding explicitly before it is used.
    NeedsReview,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PurposeBinding {
    pub purpose: Purpose,
    pub enabled: bool,
    pub primary_resource_id: Option<String>,
    #[serde(default)]
    pub fallback_resource_ids: Vec<String>,
    /// Explicit per-purpose cloud permission. Absent in older snapshots means
    /// the existing, deliberately selected cloud configuration is preserved.
    #[serde(default = "cloud_allowed_default")]
    pub cloud_allowed: bool,
    pub timeout_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_timeout_ms: Option<u64>,
    /// Legacy selection preserved while `review` is `NeedsReview`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored_primary_resource_id: Option<String>,
    pub review: BindingReview,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistrySnapshot {
    pub connections: Vec<ServiceConnection>,
    pub resources: Vec<ServiceResource>,
    pub bindings: Vec<PurposeBinding>,
}

impl RegistrySnapshot {
    pub fn connection(&self, id: &str) -> Option<&ServiceConnection> {
        self.connections.iter().find(|c| c.connection_id == id)
    }

    pub fn resource(&self, id: &str) -> Option<&ServiceResource> {
        self.resources.iter().find(|r| r.resource_id == id)
    }

    pub fn binding(&self, purpose: Purpose) -> Option<&PurposeBinding> {
        self.bindings.iter().find(|b| b.purpose == purpose)
    }
}

fn cloud_allowed_default() -> bool {
    true
}
