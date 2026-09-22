use serde::{Deserialize, Serialize};

fn default_conversation_reasoning_effort() -> String {
    "medium".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OpenAiCompatibleProviderSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request_options: Option<saaa_larm_session::http_api::LlmOptions>,
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
pub(crate) struct AgentSessionProviderSettings {
    pub(crate) id: String,
    pub(crate) enabled: bool,
    pub(crate) label: String,
    pub(crate) location: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
    pub(crate) models_path: String,
    pub(crate) sessions_path: String,
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
    #[serde(default = "default_tts_format")]
    pub(crate) response_format: String,
    pub(crate) authentication: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) speed: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pitch_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) intonation_scale: Option<f64>,
}

pub(crate) fn default_tts_format() -> String {
    "wav".to_string()
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
pub(crate) struct DynamicLanProviderSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) request_options: Option<saaa_larm_session::http_api::LlmOptions>,
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
    #[serde(rename = "agent-session")]
    AgentSession(AgentSessionProviderSettings),
    #[serde(rename = "cloud-asr")]
    CloudAsr(CloudAsrProviderSettings),
    #[serde(rename = "cloud-tts")]
    CloudTts(CloudTtsProviderSettings),
    #[serde(rename = "system-tts")]
    SystemTts(SystemTtsProviderSettings),
    #[serde(rename = "dynamic-lan")]
    DynamicLan(DynamicLanProviderSettings),
}

impl ModelProviderSettings {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.id,
            Self::AgentSession(provider) => &provider.id,
            Self::CloudAsr(provider) => &provider.id,
            Self::CloudTts(provider) => &provider.id,
            Self::SystemTts(provider) => &provider.id,
            Self::DynamicLan(provider) => &provider.id,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        match self {
            Self::OpenAiCompatible(provider) => provider.enabled,
            Self::AgentSession(provider) => provider.enabled,
            Self::CloudAsr(provider) => provider.enabled,
            Self::CloudTts(provider) => provider.enabled,
            Self::SystemTts(provider) => provider.enabled,
            Self::DynamicLan(provider) => provider.enabled,
        }
    }

    pub(crate) fn label(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.label,
            Self::AgentSession(provider) => &provider.label,
            Self::CloudAsr(provider) => &provider.label,
            Self::CloudTts(provider) => &provider.label,
            Self::SystemTts(provider) => &provider.label,
            Self::DynamicLan(provider) => &provider.label,
        }
    }

    pub(crate) fn location(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.location,
            Self::AgentSession(provider) => &provider.location,
            Self::CloudAsr(provider) => &provider.location,
            Self::CloudTts(provider) => &provider.location,
            Self::SystemTts(provider) => &provider.location,
            Self::DynamicLan(provider) => &provider.location,
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible(_) => "openai-compatible",
            // Agent Session providers own and release a remote session per SAAA
            // attempt. Persist them under the direct-provider session kind until
            // the diagnostics schema has a distinct non-allocation kind.
            Self::AgentSession(_) => "openai-compatible",
            Self::CloudAsr(_) => "cloud-asr",
            Self::CloudTts(_) => "cloud-tts",
            Self::SystemTts(_) => "system-tts",
            // A resolved dynamic_lan descriptor executes through the OpenAI-compatible
            // data plane; keep the persisted session kind compatible with the
            // existing provider-session schema.
            Self::DynamicLan(_) => "openai-compatible",
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        match self {
            Self::OpenAiCompatible(provider) => provider.enabled = enabled,
            Self::AgentSession(provider) => provider.enabled = enabled,
            Self::CloudAsr(provider) => provider.enabled = enabled,
            Self::CloudTts(provider) => provider.enabled = enabled,
            Self::SystemTts(provider) => provider.enabled = enabled,
            Self::DynamicLan(provider) => provider.enabled = enabled,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct HarnessSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) larm_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tts_voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tts_style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tts_speed: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tts_pitch_scale: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tts_intonation_scale: Option<f64>,
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
