#![allow(private_interfaces)]
//! Conversation-provider backed extraction. It resolves the already-selected conversation
//! provider from the existing settings and sends one tool-less structured request; it never
//! introduces a second provider configuration or sends the conversation to a new service.

use async_trait::async_trait;
use std::sync::Arc;

use super::contracts::*;
use super::extraction::{CorrectionExtractor, ExtractionRequest};
use super::feedback::{parse_extraction, ExtractionFailure, ParsedExtraction};
use crate::persistence::SqliteWriter;

pub struct ConversationProviderExtractor {
    writer: Arc<SqliteWriter>,
}

impl ConversationProviderExtractor {
    pub fn new(writer: Arc<SqliteWriter>) -> Self {
        Self { writer }
    }

    fn provider(&self) -> Option<crate::OpenAiCompatibleProviderSettings> {
        self.writer
            .read_serialized(|connection| {
                let providers = crate::persistence::load_model_providers(connection)
                    .map_err(|error| error.to_string())?;
                let route = crate::persistence::load_routing_settings(connection)
                    .map_err(|error| error.to_string())?
                    .conversation_respond;
                if route.source == "harness" {
                    return Ok(None);
                }
                let configured = providers.providers.iter().find(|provider| {
                    Some(provider.id()) == route.primary_provider_id.as_deref()
                        && provider.enabled()
                });
                Ok(configured.and_then(|provider| match provider {
                    crate::ModelProviderSettings::OpenAiCompatible(settings) => {
                        Some(settings.clone())
                    }
                    _ => None,
                }))
            })
            .ok()
            .flatten()
    }
}

/// Builds a bounded, always-valid JSON extraction prompt. The full eligible set is never shown;
/// only the recently selected tool names are, and older decision summaries are dropped until the
/// encoded payload fits the documented 16 KiB budget.
pub fn extraction_user_prompt(request: &ExtractionRequest) -> String {
    let user_message = repository_truncate(&request.user_message, EXTRACT_USER_MESSAGE_MAX_BYTES);
    let mut allowed_decisions: Vec<&String> = request.allowed_decisions.iter().collect();
    allowed_decisions.sort();
    let decisions: Vec<&super::extraction::RecentDecision> = request
        .recent_decisions
        .iter()
        .take(EXTRACT_RECENT_DECISIONS)
        .collect();
    for keep in (0..=decisions.len()).rev() {
        let recent: Vec<serde_json::Value> = decisions[..keep]
            .iter()
            .map(|decision| {
                serde_json::json!({
                    "decisionId": decision.decision_id,
                    "scenario": decision.scenario_summary,
                    "tools": decision.tool_ids,
                })
            })
            .collect();
        let payload = serde_json::json!({
            "userMessage": user_message,
            "recentDecisions": recent,
            "selectedToolNames": &request.prompt_tools,
            "allowedDecisionIds": allowed_decisions,
        });
        let encoded = payload.to_string();
        if encoded.len() <= EXTRACT_INPUT_MAX_BYTES {
            return encoded;
        }
    }
    serde_json::json!({
        "userMessage": user_message,
        "selectedToolNames": &request.prompt_tools,
    })
    .to_string()
}

fn repository_truncate(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Strips an optional markdown fence so a provider that wraps JSON still parses, while keeping
/// the strict host validation downstream.
pub fn extract_json_body(raw: &str) -> &str {
    let trimmed = raw.trim();
    let without_fence = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    without_fence
        .strip_suffix("```")
        .unwrap_or(without_fence)
        .trim()
}

#[async_trait]
impl CorrectionExtractor for ConversationProviderExtractor {
    async fn extract(
        &self,
        request: ExtractionRequest,
    ) -> Result<ParsedExtraction, ExtractionFailure> {
        let provider = self.provider().ok_or(ExtractionFailure::UnexpectedShape)?;
        let user = extraction_user_prompt(&request);
        let raw = crate::providers::openai_compatible::complete_tool_less(
            &provider,
            crate::providers::openai_compatible::EXTRACTION_SYSTEM_PROMPT,
            &user,
        )
        .await
        .map_err(|_| ExtractionFailure::UnexpectedShape)?;
        let body = extract_json_body(&raw);
        parse_extraction(
            body,
            &request.user_message,
            &request.allowed_decisions,
            &request.allowed_tools,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_are_stripped() {
        assert_eq!(extract_json_body("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(extract_json_body("{\"a\":1}"), "{\"a\":1}");
    }

    #[test]
    fn prompt_is_valid_json_and_bounded_even_with_many_decisions() {
        let request = ExtractionRequest {
            user_message: "あ".repeat(20_000),
            recent_decisions: (0..20)
                .map(|index| super::super::extraction::RecentDecision {
                    decision_id: format!("d{index}"),
                    scenario_summary: "x".repeat(500),
                    tool_ids: (0..40).map(|tool| format!("tool_{tool}")).collect(),
                })
                .collect(),
            allowed_decisions: (0..20).map(|index| format!("d{index}")).collect(),
            allowed_tools: (0..500).map(|tool| format!("tool_{tool}")).collect(),
            prompt_tools: (0..40).map(|tool| format!("tool_{tool}")).collect(),
        };
        let prompt = extraction_user_prompt(&request);
        assert!(
            prompt.len() <= EXTRACT_INPUT_MAX_BYTES,
            "len={}",
            prompt.len()
        );
        assert!(serde_json::from_str::<serde_json::Value>(&prompt).is_ok());
    }

    #[test]
    fn prompt_is_bounded() {
        let request = ExtractionRequest {
            user_message: "x".repeat(1000),
            recent_decisions: Vec::new(),
            allowed_decisions: std::collections::HashSet::new(),
            allowed_tools: std::collections::HashSet::new(),
            prompt_tools: vec!["minutes".to_string()],
        };
        let prompt = extraction_user_prompt(&request);
        assert!(prompt.len() <= EXTRACT_INPUT_MAX_BYTES);
        assert!(serde_json::from_str::<serde_json::Value>(&prompt).is_ok());
    }
}
