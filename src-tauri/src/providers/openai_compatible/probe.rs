use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};

use super::{provider_api_key, provider_models_url};
use crate::OpenAiCompatibleProviderSettings;

pub(crate) async fn probe_model_provider(
    provider: &OpenAiCompatibleProviderSettings,
) -> Result<String, String> {
    let mut client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none());
    if provider.location == "local" {
        client = client.no_proxy();
    }
    let client = client
        .build()
        .map_err(|error| format!("Could not initialize HTTP client: {error}"))?;
    let api_key = provider_api_key(provider)?;
    if provider.authentication == "api-key" && api_key.is_none() {
        return Err("API key is not configured in macOS Keychain".to_string());
    }
    let mut request = client.get(provider_models_url(&provider.endpoint)?);
    if let Some(api_key) = api_key.as_deref() {
        request = request.bearer_auth(api_key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Connection failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Provider returned HTTP {}", response.status()));
    }
    let models: ModelsResponse = serde_json::from_slice(&read_probe_body(response).await?)
        .map_err(|_| "Provider returned an invalid /v1/models response".to_string())?;
    if !models.data.iter().any(|model| model.id == provider.model) {
        return Err(format!(
            "Configured model {} is not listed by the provider",
            provider.model
        ));
    }

    let stream_url = crate::providers::stream::provider_stream_url(&provider.endpoint)
        .map_err(|_| "Provider does not advertise a valid LLM WebSocket endpoint".to_string())?;
    let authorization = api_key.as_deref().map(|value| format!("Bearer {value}"));
    let messages = vec![
        json!({ "role": "system", "content": "Reply briefly." }),
        json!({ "role": "user", "content": "Connectivity check" }),
    ];
    let input = crate::StartTurnInput {
        run_id: format!("probe_{}", uuid::Uuid::new_v4().simple()),
        conversation_id: "provider_probe".to_string(),
        content: "Connectivity check".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let sink = tauri::ipc::Channel::new(|_| Ok(()));
    let result = crate::providers::llm_websocket::client::run(
        crate::providers::llm_websocket::client::WebSocketRunContext {
            stream_url: stream_url.as_str(),
            authorization: authorization.as_deref(),
            allocation_id: None,
            model: &provider.model,
            messages: &messages,
            tools: &[],
            reasoning_effort: crate::providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            max_output_tokens: 64,
            tool_timeout: Duration::from_secs(10),
            timeout: Duration::from_secs(10),
            input: &input,
            on_event: &sink,
            cancellation: Arc::new(crate::RunCancellation::default()),
            output_persistence: None,
        },
    )
    .await
    .map_err(|error| format!("WebSocket generation probe failed: {error:?}"))?;
    if !matches!(
        result,
        crate::providers::llm_websocket::client::WebSocketRunResult::Completed(ref content)
            if !content.trim().is_empty()
    ) {
        return Err("Provider WebSocket probe did not complete a text response".to_string());
    }
    Ok(format!(
        "Model {} is listed and completed a generation probe",
        provider.model
    ))
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelDescriptor>,
}

#[derive(Debug, Deserialize)]
struct ModelDescriptor {
    id: String,
}

async fn read_probe_body(response: reqwest::Response) -> Result<Vec<u8>, String> {
    const LIMIT: usize = 1_048_576;
    if response
        .content_length()
        .is_some_and(|length| length > LIMIT as u64)
    {
        return Err("Provider probe response exceeded the size limit".to_string());
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("Could not read provider response: {error}"))?;
        if body.len().saturating_add(chunk.len()) > LIMIT {
            return Err("Provider probe response exceeded the size limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
