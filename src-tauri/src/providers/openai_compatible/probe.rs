use std::sync::Arc;

use crate::OpenAiCompatibleProviderSettings;

pub(crate) async fn probe_model_provider(
    provider: &OpenAiCompatibleProviderSettings,
) -> Result<String, String> {
    probe_model_provider_with_api_key(provider, None).await
}

pub(crate) async fn probe_model_provider_with_api_key(
    provider: &OpenAiCompatibleProviderSettings,
    api_key: Option<&str>,
) -> Result<String, String> {
    let input = crate::StartTurnInput {
        run_id: format!("probe_{}", uuid::Uuid::new_v4().simple()),
        conversation_id: "provider_probe".to_string(),
        content: "Connectivity check".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let sink = tauri::ipc::Channel::new(|_| Ok(()));
    let history = [crate::ipc_contract::ConversationMessage {
        parts: None,
        id: "probe-message".into(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: input.content.clone(),
        created_at: String::new(),
    }];
    let stored_key = if api_key.is_none() {
        super::provider_api_key(provider)?
    } else {
        None
    };
    let key = api_key.or(stored_key.as_deref().map(String::as_str));
    if provider.authentication == "api-key" && key.is_none() {
        return Err("API key is not configured in the operating system credential store".into());
    }
    let authorization = key.map(|key| format!("Bearer {key}"));
    let result = crate::providers::chat_completions::run_with_options(
        &provider.endpoint,
        authorization.as_deref(),
        &provider.model,
        &history,
        10_000,
        crate::providers::stream::ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 64,
            input: &input,
            on_event: &sink,
            cancellation: Arc::new(crate::RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
        crate::providers::chat_completions::RequestMode::JsonProbe,
        &provider
            .request_options
            .clone()
            .unwrap_or_else(saaa_larm_session::http_api::LlmOptions::standard),
    )
    .await;
    match result {
        Ok(content) if !content.trim().is_empty() => {
            Ok("Provider completed an HTTP Chat Completions generation probe".to_string())
        }
        Err(crate::providers::stream::ProviderAttemptError::Failed { kind, .. }) => {
            Err(kind.public_message().as_str().to_string())
        }
        _ => Err("Provider did not complete the HTTP generation probe".to_string()),
    }
}
