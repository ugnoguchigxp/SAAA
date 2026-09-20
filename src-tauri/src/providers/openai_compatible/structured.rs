//! One-shot, tool-less structured completion against the configured conversation provider.
//!
//! This is the extraction transport: it reuses the same URL/auth/model resolution and HTTP layer
//! as the conversation path (no second provider stack), disables tools, and returns the assistant
//! text. It is not a second conversation runtime.

use std::sync::Arc;

use crate::ipc_contract::ConversationMessage;
use crate::OpenAiCompatibleProviderSettings;
use crate::RunCancellation;

const EXTRACTION_TIMEOUT_MS: u64 = 5_000;
const EXTRACTION_MAX_TOKENS: u32 = 700;
/// Sends one tool-less Chat Completions request and returns the assistant content. The caller
/// enforces the output size and schema.
pub(crate) async fn complete_tool_less(
    provider: &OpenAiCompatibleProviderSettings,
    system: &str,
    user: &str,
) -> Result<String, String> {
    let stored_key = super::provider_api_key(provider)?;
    let authorization = stored_key.map(|key| format!("Bearer {}", key.as_str()));
    if provider.authentication == "api-key" && authorization.is_none() {
        return Err("API key is not configured".to_string());
    }
    let history = [
        ConversationMessage {
            parts: None,
            id: "extraction-system".into(),
            conversation_id: "tool_selection_extraction".into(),
            role: "system".into(),
            content: system.to_string(),
            created_at: String::new(),
        },
        ConversationMessage {
            parts: None,
            id: "extraction-user".into(),
            conversation_id: "tool_selection_extraction".into(),
            role: "user".into(),
            content: user.to_string(),
            created_at: String::new(),
        },
    ];
    let input = crate::StartTurnInput {
        run_id: format!("extract_{}", uuid::Uuid::new_v4().simple()),
        conversation_id: "tool_selection_extraction".to_string(),
        content: user.to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let sink = tauri::ipc::Channel::new(|_| Ok(()));
    let mut options = provider
        .request_options
        .clone()
        .unwrap_or_else(saaa_larm_session::http_api::LlmOptions::standard);
    // Fixed extraction sampling: deterministic, no provider-default temperature.
    options.temperature = Some(0.0);
    crate::providers::chat_completions::run_with_options(
        &provider.endpoint,
        authorization.as_deref(),
        &provider.model,
        &history,
        EXTRACTION_TIMEOUT_MS,
        crate::providers::stream::ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: EXTRACTION_MAX_TOKENS,
            input: &input,
            on_event: &sink,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
        crate::providers::chat_completions::RequestMode::JsonProbe,
        &options,
    )
    .await
    .map_err(|_| "provider extraction failed".to_string())
    .and_then(|content| {
        if content.trim().is_empty() {
            Err("provider extraction returned no content".to_string())
        } else {
            Ok(content)
        }
    })
}

/// Fixed system instruction. Kept here so the conversation path and the worker path cannot drift.
pub(crate) const EXTRACTION_SYSTEM_PROMPT: &str = "You extract tool-selection intent and user \
corrections from one user message. Rules: if there is no correction, return feedback=[]. Never \
treat silence or an ordinary request as agreement or positive feedback. Never widen the scope \
beyond the scope the host supplied. Never turn a complaint about arguments or output into a \
negation of the whole tool. Return only the fixed JSON object with keys scenario and feedback; \
no prose, no markdown.";

#[cfg(test)]
mod tests {
    #[test]
    fn system_prompt_keeps_the_fixed_constraints() {
        let prompt = super::EXTRACTION_SYSTEM_PROMPT;
        assert!(prompt.contains("feedback=[]"));
        assert!(prompt.contains("Never"));
        assert!(prompt.contains("widen the scope"));
    }
}
