//! One-shot generation-only tool-less completion (plan 12.3).
//!
//! It reuses the same provider resolution and HTTP path as extraction but applies the generation
//! output ceiling (4096 tokens) and outer deadline. The existing extraction limits are unchanged.

use std::sync::Arc;

use crate::ipc_contract::ConversationMessage;
use crate::{OpenAiCompatibleProviderSettings, RunCancellation};

const GENERATION_TIMEOUT_MS: u64 = 90_000;
const GENERATION_MAX_TOKENS: u32 = 4096;

#[allow(dead_code)]
/// One generation-only tool-less completion. It reuses the same provider resolution and HTTP path
/// as extraction but applies the generation output ceiling and outer deadline.
pub(crate) async fn complete_generation(
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
            id: "generation-system".into(),
            conversation_id: "capability_generation".into(),
            role: "system".into(),
            content: system.to_string(),
            created_at: String::new(),
        },
        ConversationMessage {
            parts: None,
            id: "generation-user".into(),
            conversation_id: "capability_generation".into(),
            role: "user".into(),
            content: user.to_string(),
            created_at: String::new(),
        },
    ];
    let input = crate::StartTurnInput {
        run_id: format!("generate_{}", uuid::Uuid::new_v4().simple()),
        conversation_id: "capability_generation".to_string(),
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
    options.temperature = Some(0.0);
    crate::providers::chat_completions::run_with_options(
        &provider.endpoint,
        authorization.as_deref(),
        &provider.model,
        &history,
        GENERATION_TIMEOUT_MS,
        crate::providers::stream::ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: GENERATION_MAX_TOKENS,
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
    .map_err(|_| "generation provider call failed".to_string())
    .and_then(|content| {
        if content.trim().is_empty() {
            Err("generation provider returned no content".to_string())
        } else {
            Ok(content)
        }
    })
}
