use serde::{Deserialize, Serialize};
use serde_json::Value;

fn default_conversation_reasoning_effort() -> String {
    "medium".to_string()
}

fn default_max_output_tokens() -> u32 {
    crate::providers::completion::DEFAULT_MAX_OUTPUT_TOKENS
}

fn default_stt_host() -> String {
    crate::voice::network_asr::DEFAULT_HOST.to_string()
}

fn default_codex_input_modalities() -> Vec<String> {
    vec!["text".to_string(), "image".to_string()]
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsDocument {
    pub(crate) namespace: String,
    pub(crate) key: String,
    pub(crate) schema_version: i64,
    pub(crate) value_json: Value,
    pub(crate) updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveSettingsDocumentInput {
    pub(crate) namespace: String,
    pub(crate) key: String,
    pub(crate) schema_version: i64,
    pub(crate) value_json: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveSettingsDocumentsInput {
    pub(crate) documents: Vec<SaveSettingsDocumentInput>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Conversation {
    pub(crate) id: String,
    pub(crate) title: Option<String>,
    pub(crate) task_mode: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateConversationInput {
    pub(crate) title: Option<String>,
    pub(crate) task_mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AppendMessageInput {
    pub(crate) conversation_id: String,
    pub(crate) role: String,
    pub(crate) content: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppSnapshot {
    pub(crate) settings: Vec<SettingsDocument>,
    pub(crate) conversations: Vec<Conversation>,
    pub(crate) primary_conversation_id: String,
    pub(crate) effective_route: EffectiveRouteSnapshot,
    pub(crate) larm_runtime: LarmRuntimeStatus,
    pub(crate) voice_profile: crate::voice::profile::VoiceProfileSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EffectiveRouteSnapshot {
    pub(crate) provider_id: Option<String>,
    pub(crate) label: String,
    pub(crate) location: Option<String>,
    pub(crate) state: String,
    pub(crate) fallback_used: bool,
    pub(crate) reason_code: String,
    pub(crate) updated_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LarmRuntimeStatus {
    pub(crate) state: &'static str,
    pub(crate) message: &'static str,
    pub(crate) contract_commit: &'static str,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexReasoningEffort {
    pub(crate) reasoning_effort: String,
    pub(crate) description: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexModelOption {
    pub(crate) id: String,
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) display_name: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) hidden: bool,
    #[serde(default)]
    pub(crate) default_reasoning_effort: Option<String>,
    #[serde(default)]
    pub(crate) supported_reasoning_efforts: Vec<CodexReasoningEffort>,
    #[serde(default = "default_codex_input_modalities")]
    pub(crate) input_modalities: Vec<String>,
    #[serde(default)]
    pub(crate) supports_personality: bool,
    #[serde(default)]
    pub(crate) is_default: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexModelPage {
    pub(crate) data: Vec<CodexModelOption>,
    pub(crate) next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CodexRuntimeStatus {
    pub(crate) installed: bool,
    pub(crate) authenticated: bool,
    pub(crate) runtime: String,
    pub(crate) account_type: Option<String>,
    pub(crate) message: String,
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
    pub(crate) credential_status: String,
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
    #[serde(rename = "larm")]
    Larm(LarmProviderSettings),
    #[serde(rename = "dynamic-lan")]
    DynamicLan(DynamicLanProviderSettings),
}

impl ModelProviderSettings {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.id,
            Self::Larm(provider) => &provider.id,
            Self::DynamicLan(provider) => &provider.id,
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        match self {
            Self::OpenAiCompatible(provider) => provider.enabled,
            Self::Larm(provider) => provider.enabled,
            Self::DynamicLan(provider) => provider.enabled,
        }
    }

    pub(crate) fn label(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.label,
            Self::Larm(provider) => &provider.label,
            Self::DynamicLan(provider) => &provider.label,
        }
    }

    pub(crate) fn location(&self) -> &str {
        match self {
            Self::OpenAiCompatible(provider) => &provider.location,
            Self::Larm(provider) => &provider.location,
            Self::DynamicLan(provider) => &provider.location,
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::OpenAiCompatible(_) => "openai-compatible",
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
            Self::Larm(provider) => provider.enabled = enabled,
            Self::DynamicLan(provider) => provider.enabled = enabled,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelProvidersSettings {
    pub(crate) providers: Vec<ModelProviderSettings>,
    #[serde(default = "default_conversation_reasoning_effort")]
    pub(crate) reasoning_effort: String,
    #[serde(default = "default_max_output_tokens")]
    pub(crate) max_output_tokens: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct ConversationRouteSettings {
    pub(crate) primary_provider_id: String,
    pub(crate) fallback_provider_ids: Vec<String>,
    pub(crate) timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct CodingRouteSettings {
    pub(crate) provider_id: String,
    pub(crate) timeout_ms: u64,
    pub(crate) read_only: bool,
    pub(crate) network_enabled: bool,
    pub(crate) web_search_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct RoutingSettings {
    pub(crate) conversation_respond: ConversationRouteSettings,
    pub(crate) coding_assist: CodingRouteSettings,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct CodexAgentRuntimeSettings {
    #[serde(default = "default_agent_name")]
    pub(crate) agent_name: String,
    #[serde(default)]
    pub(crate) user_name: String,
    pub(crate) enabled: bool,
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) runtime_mode: String,
    pub(crate) health: String,
    pub(crate) sandbox_mode: String,
    pub(crate) approval_policy: String,
    pub(crate) network_enabled: bool,
    pub(crate) web_search_enabled: bool,
    pub(crate) workspace_policy: String,
}

fn default_agent_name() -> String {
    crate::DEFAULT_AGENT_NAME.to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct VoiceRuntimeSettings {
    pub(crate) input_device_id: String,
    pub(crate) capture_mode: String,
    #[serde(default = "default_stt_host")]
    pub(crate) stt_host: String,
    pub(crate) stt_provider_id: String,
    pub(crate) stt_model: String,
    pub(crate) tts_provider_id: String,
    pub(crate) tts_voice: String,
    pub(crate) auto_speak: bool,
    pub(crate) cloud_fallback_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct SecurityRuntimeSettings {
    pub(crate) credential_storage: String,
    pub(crate) local_only_when_selected: bool,
    pub(crate) diagnostics_redaction: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartTurnInput {
    pub(crate) run_id: String,
    pub(crate) conversation_id: String,
    pub(crate) content: String,
    pub(crate) workspace_path: Option<String>,
    #[serde(default)]
    pub(crate) retry_input_message_id: Option<String>,
    pub(crate) input_origin: String,
    pub(crate) presentation_mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TestProviderInput {
    pub(crate) provider: ModelProviderSettings,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProviderTestResult {
    pub(crate) provider_id: String,
    pub(crate) ok: bool,
    pub(crate) message: String,
    pub(crate) latency_ms: u128,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResolveNetworkAsrInput {
    pub(crate) host: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NetworkAsrResolution {
    pub(crate) provider_id: String,
    pub(crate) endpoint: String,
    pub(crate) model: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalArtifactResult {
    pub(crate) path: String,
    pub(crate) created_at: String,
}
