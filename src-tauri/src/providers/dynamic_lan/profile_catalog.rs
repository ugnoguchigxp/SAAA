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
    use serde_json::json;

    #[test]
    fn v3_catalog_keeps_provider_context_windows() {
        let catalog: AgentProfileCatalog = serde_json::from_value(json!({
            "contractVersion":"agent-connection.v3",
            "profiles":[{
                "id":"saaa-conversation-ornith15",
                "providers":[
                    {"name":"asr","capability":"speech.stt","protocol":"openai.audio-transcriptions.v1","model":"qwen3-asr-1.7b"},
                    {"name":"llm","capability":"llm.general","protocol":"openai.chat-completions.v1","model":"ornith-1.5-35b",
                        "contextWindow":{"maxTokens":131072,"outputReserveTokens":4096,"safetyMarginTokens":1976}}
                ]
            }],
            "audiences":["saaa-desktop"]
        }))
        .unwrap();
        assert_eq!(catalog.contract_version, "agent-connection.v3");
        let llm = catalog.profiles[0]
            .providers
            .iter()
            .find(|provider| provider.name == "llm")
            .unwrap();
        assert_eq!(llm.context_window.unwrap().max_tokens, 131_072);
        assert!(catalog.profiles[0]
            .providers
            .iter()
            .any(|provider| provider.name == "asr" && provider.context_window.is_none()));
    }

    #[test]
    fn health_requires_real_capacity_even_in_test_builds() {
        let result = serde_json::from_value::<crate::providers::dynamic_lan::ProviderHealth>(
            json!({"ready":true,"acceptingRequests":true}),
        );
        assert!(result.unwrap_err().to_string().contains("capacity"));
    }
}
