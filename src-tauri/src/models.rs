use serde::{Deserialize, Serialize};
use serde_json::Value;

mod provider_settings;
pub(crate) use provider_settings::*;

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetVoiceListeningEnabledInput {
    pub(crate) enabled: bool,
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

#[cfg(test)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateConversationInput {
    pub(crate) title: Option<String>,
    pub(crate) task_mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ListMessagesInput {
    pub(crate) conversation_id: String,
    pub(crate) cursor: Option<String>,
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct ConversationRouteSettings {
    pub(crate) source: String,
    pub(crate) primary_provider_id: Option<String>,
    pub(crate) fallback_provider_ids: Vec<String>,
    pub(crate) timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct VoiceRouteSettings {
    pub(crate) source: String,
    pub(crate) provider_id: Option<String>,
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
    pub(crate) voice_transcribe: VoiceRouteSettings,
    pub(crate) voice_speak: VoiceRouteSettings,
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
    pub(crate) listening_enabled: bool,
    pub(crate) input_device_id: String,
    pub(crate) output_device_id: String,
    pub(crate) vad_sensitivity: String,
    pub(crate) silence_timeout_ms: u32,
    #[serde(default = "crate::voice::language::default_allowed_languages")]
    pub(crate) allowed_languages: Vec<String>,
    pub(crate) auto_speak: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(crate) struct SecurityRuntimeSettings {
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
    #[serde(default)]
    pub(crate) source_id: Option<String>,
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
