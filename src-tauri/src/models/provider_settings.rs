use serde::{Deserialize, Serialize};

fn default_conversation_reasoning_effort() -> String {
    "medium".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OpenAiCompatibleProviderSettings {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) label: String,
    pub(crate) location: String,
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) authentication: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CloudAsrProviderSettings {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) label: String,
    pub(crate) location: String,
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) language: String,
    pub(crate) authentication: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CloudTtsProviderSettings {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) label: String,
    pub(crate) location: String,
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) voice: String,
    pub(crate) authentication: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SystemTtsProviderSettings {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) label: String,
    pub(crate) location: String,
    pub(crate) voice: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LarmProviderSettings {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) label: String,
    pub(crate) location: String,
    pub(crate) base_url: String,
    pub(crate) token_env: String,
    pub(crate) allocation_ttl_seconds: u32,
    pub(crate) allocation_startup_timeout_seconds: u32,
    pub(crate) allow_fallback_by_default: bool,
    pub(crate) deployment_policy: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DynamicLanProviderSettings {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) label: String,
    pub(crate) location: String,
    pub(crate) host: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(crate) enum ModelProviderSettings {
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible(OpenAiCompatibleProviderSettings),
    #[serde(rename = "cloud-asr")]
    CloudAsr(CloudAsrProviderSettings),
    #[serde(rename = "cloud-tts")]
    CloudTts(CloudTtsProviderSettings),
    #[serde(rename = "system-tts")]
    SystemTts(SystemTtsProviderSettings),
    #[serde(rename = "larm")]
    Larm(LarmProviderSettings),
    #[serde(rename = "dynamic-lan")]
    DynamicLan(DynamicLanProviderSettings),
}

impl ModelProviderSettings {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.id,
            Self::CloudAsr(provider) => &provider.id,
            Self::CloudTts(provider) => &provider.id,
            Self::SystemTts(provider) => &provider.id,
            Self::Larm(provider) => &provider.id,
            Self::DynamicLan(provider) => &provider.id,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        match self {
            Self::OpenAiCompatible(provider) => provider.enabled,
            Self::CloudAsr(provider) => provider.enabled,
            Self::CloudTts(provider) => provider.enabled,
            Self::SystemTts(provider) => provider.enabled,
            Self::Larm(provider) => provider.enabled,
            Self::DynamicLan(provider) => provider.enabled,
        }
    }

    pub(crate) fn label(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.label,
            Self::CloudAsr(provider) => &provider.label,
            Self::CloudTts(provider) => &provider.label,
            Self::SystemTts(provider) => &provider.label,
            Self::Larm(provider) => &provider.label,
            Self::DynamicLan(provider) => &provider.label,
        }
    }

    pub(crate) fn location(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.location,
            Self::CloudAsr(provider) => &provider.location,
            Self::CloudTts(provider) => &provider.location,
            Self::SystemTts(provider) => &provider.location,
            Self::Larm(provider) => &provider.location,
            Self::DynamicLan(provider) => &provider.location,
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible(_) => "openai-compatible",
            Self::CloudAsr(_) => "cloud-asr",
            Self::CloudTts(_) => "cloud-tts",
            Self::SystemTts(_) => "system-tts",
            Self::Larm(_) => "larm",
            // A resolved dynamic_lan descriptor executes through the OpenAI-compatible
            // data plane; keep the persisted session kind compatible with the
            // existing provider-session schema.
            Self::DynamicLan(_) => "openai-compatible",
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        match self {
            Self::OpenAiCompatible(provider) => provider.enabled = enabled,
            Self::CloudAsr(provider) => provider.enabled = enabled,
            Self::CloudTts(provider) => provider.enabled = enabled,
            Self::SystemTts(provider) => provider.enabled = enabled,
            Self::Larm(provider) => provider.enabled = enabled,
            Self::DynamicLan(provider) => provider.enabled = enabled,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HarnessSettings {
    pub(crate) address: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelProvidersSettings {
    pub(crate) harness: HarnessSettings,
    pub(crate) providers: Vec<ModelProviderSettings>,
    #[serde(default = "default_conversation_reasoning_effort")]
    pub(crate) reasoning_effort: String,
}
