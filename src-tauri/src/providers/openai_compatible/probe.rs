use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

use super::{drain_sse_events, provider_api_key, provider_chat_url, provider_models_url};
use crate::providers::completion::{thinking_enabled, CompletionFinish, CompletionTerminal};
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
    let mut request = client.get(provider_models_url(&provider.endpoint)?);
    if let Some(api_key) = provider_api_key(provider) {
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

    let mut body = json!({
        "model": provider.model,
        "messages": [
            { "role": "system", "content": "Reply briefly." },
            { "role": "user", "content": "Connectivity check" }
        ],
        "stream": true,
        "reasoning_effort": crate::providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
        "max_tokens": 64
    });
    if provider.location == "local" {
        body["chat_template_kwargs"] = json!({
            "enable_thinking": thinking_enabled(
                crate::providers::DEFAULT_CONVERSATION_REASONING_EFFORT
            )
        });
    }
    let mut request = client
        .post(provider_chat_url(&provider.endpoint)?)
        .json(&body);
    if let Some(api_key) = provider_api_key(provider) {
        request = request.bearer_auth(api_key);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Generation probe failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Provider generation probe returned HTTP {}",
            response.status()
        ));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !content_type.starts_with("text/event-stream") {
        return Err("Provider generation probe did not return an SSE stream".to_string());
    }
    validate_probe_stream(read_probe_body(response).await?)?;
    Ok(format!(
        "Model {} is listed and completed a generation probe",
        provider.model
    ))
}

fn validate_probe_stream(mut body: Vec<u8>) -> Result<(), String> {
    if !body.ends_with(b"\n\n") && !body.ends_with(b"\r\n\r\n") {
        body.extend_from_slice(b"\n\n");
    }
    let events = drain_sse_events(&mut body, 1_048_576)
        .map_err(|_| "Provider generation probe returned invalid SSE framing".to_string())?;
    let mut terminal = CompletionTerminal::default();
    let mut content_seen = false;
    let mut done_seen = false;
    for event in events {
        let Some(data) = event.lines().find_map(|line| line.strip_prefix("data:")) else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            done_seen = true;
            break;
        }
        let value: serde_json::Value = serde_json::from_str(data)
            .map_err(|_| "Provider generation probe returned invalid event JSON".to_string())?;
        terminal.observe(&value).map_err(|_| {
            "Provider generation probe returned an invalid finish reason".to_string()
        })?;
        content_seen |= value
            .pointer("/choices/0/delta/content")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|content| !content.trim().is_empty());
    }
    if !done_seen
        || terminal.complete().map_err(|_| {
            "Provider generation probe ended without a terminal finish reason".to_string()
        })? != CompletionFinish::Stop
        || !content_seen
    {
        return Err("Provider generation probe did not complete a text response".to_string());
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::validate_probe_stream;

    #[test]
    fn production_probe_requires_text_stop_and_done() {
        let valid = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        assert!(validate_probe_stream(valid.as_bytes().to_vec()).is_ok());

        for invalid in [
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"unterminated\"}}]}\n\ndata: [DONE]\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ] {
            assert!(validate_probe_stream(invalid.as_bytes().to_vec()).is_err());
        }
    }
}
