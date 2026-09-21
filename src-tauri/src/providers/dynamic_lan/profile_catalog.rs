//! Discovery wire types. Deserialization is identical in tests and the desktop app.
//! v3 budgets belong to individual providers; unrelated ASR/TTS entries need none.
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentProfileCatalog {
    pub contract_version: String,
    pub default_agent_profile: Option<String>,
    pub profiles: Vec<CatalogAgentProfile>,
    pub audiences: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct CatalogAgentProfile {
    pub id: String,
    /// Only the legacy v1 contract places a budget at profile level.
    #[serde(rename = "contextWindow")]
    pub legacy_profile_context_window: Option<ProviderContextWindow>,
    pub providers: Vec<CatalogProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CatalogProvider {
    pub name: String,
    pub capability: String,
    #[serde(default)]
    pub supported_capabilities: Vec<String>,
    pub protocol: String,
    pub model: String,
    pub context_window: Option<ProviderContextWindow>,
}

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProviderContextWindow {
    pub max_tokens: u32,
    pub output_reserve_tokens: u32,
    pub safety_margin_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::dynamic_lan::validate::select_default_llm_profile;
    use serde_json::json;

    fn v3_catalog() -> serde_json::Value {
        json!({
            "contractVersion":"agent-connection.v3", "defaultAgentProfile":"coding-default",
            "audiences":["saaa-desktop"], "profiles":[
                {"id":"asr-qwen", "providers":[{"name":"asr","capability":"speech.stt",
                    "protocol":"openai.audio-transcriptions.v1","model":"qwen-asr"}]},
                {"id":"coding-default", "providers":[{"name":"llm","capability":"llm.coding",
                    "supportedCapabilities":["llm.coding","llm.reasoning"],
                    "protocol":"openai.chat-completions.v1","model":"qwen3.8",
                    "contextWindow":{"maxTokens":230400,"outputReserveTokens":4096,"safetyMarginTokens":1976}}]}
            ]
        })
    }

    #[test]
    fn reads_provider_budget_without_requiring_budgets_for_asr() {
        let catalog: AgentProfileCatalog = serde_json::from_value(v3_catalog()).unwrap();
        let selected = select_default_llm_profile(&catalog).unwrap();
        assert_eq!(selected.model, "qwen3.8");
        assert_eq!(selected.context_window.max_tokens, 230400);
        assert_eq!(selected.context_window.safety_margin_tokens, 1976);
    }

    #[test]
    fn v3_rejects_profile_level_budget_instead_of_inventing_a_default() {
        let mut wire = v3_catalog();
        let budget = wire["profiles"][1]["providers"][0]
            .as_object_mut()
            .unwrap()
            .remove("contextWindow")
            .unwrap();
        wire["profiles"][1]["contextWindow"] = budget;
        let catalog = serde_json::from_value(wire).unwrap();
        let error = select_default_llm_profile(&catalog).unwrap_err();
        assert_eq!(error.code(), Some("harness-llm-context-window-missing"));
    }

    #[test]
    fn rejects_invalid_selected_budget_with_actionable_code() {
        let mut wire = v3_catalog();
        wire["profiles"][1]["providers"][0]["contextWindow"]["maxTokens"] = json!(0);
        let catalog = serde_json::from_value(wire).unwrap();
        assert_eq!(
            select_default_llm_profile(&catalog).unwrap_err().code(),
            Some("harness-llm-context-window-invalid")
        );
    }

    #[test]
    fn health_requires_real_capacity_even_in_test_builds() {
        let result = serde_json::from_value::<crate::providers::dynamic_lan::ProviderHealth>(
            json!({"ready":true,"acceptingRequests":true}),
        );
        assert!(result.unwrap_err().to_string().contains("capacity"));
    }
}
