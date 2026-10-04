//! Bounded Messages adapter; the host still owns tools and answer adoption.
//! https://platform.claude.com/docs/en/api/overview
//! Complete-response mode deliberately publishes text only after end_turn.
use crate::{
    providers::stream::{ProviderAttemptError, ProviderFailureKind as Failure},
    RunCancellation,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub(crate) struct Completion {
    pub(crate) content: String,
    pub(crate) model: Option<String>,
    pub(crate) request_id: Option<String>,
    pub(crate) usage: Value,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    endpoint: &str,
    key: Option<&str>,
    model: &str,
    system: &str,
    recent: &[(String, String)],
    text: &str,
    timeout_ms: u64,
    max_tokens: u32,
    cancellation: Arc<RunCancellation>,
) -> Result<Completion, ProviderAttemptError> {
    let fail = |kind| ProviderAttemptError::failed(kind, false);
    // Only a bare origin or explicit base prefix is accepted. operation_url
    // rejects userinfo, query and fragments; credentials never follow redirects.
    let endpoint = endpoint
        .trim_end_matches('/')
        .strip_suffix("/messages")
        .unwrap_or(endpoint);
    let url = saaa_larm_session::http_api::operation_url(endpoint, "messages")
        .map_err(|_| fail(Failure::Contract))?;
    let mut messages = Vec::new();
    for (role, content) in recent {
        if !matches!(role.as_str(), "user" | "assistant") {
            return Err(fail(Failure::Contract));
        }
        messages.push(json!({"role":role,"content":content}));
    }
    messages.push(json!({"role":"user","content":text}));
    let body = json!({"model":model,"system":system,"messages":messages,"max_tokens":max_tokens,"stream":false});
    if body.to_string().len() > 96_000 {
        return Err(fail(Failure::RequestTooLarge));
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| fail(Failure::Internal))?;
    let mut request = client
        .post(url)
        .header("anthropic-version", "2023-06-01")
        .json(&body);
    if let Some(key) = key {
        request = request.header("x-api-key", key);
    }
    let work = async {
        let mut response = request.send().await.map_err(|e| {
            fail(if e.is_connect() {
                Failure::Connect
            } else {
                Failure::Network
            })
        })?;
        if !response.status().is_success() {
            return Err(fail(if response.status().as_u16() == 529 {
                Failure::Unavailable
            } else {
                crate::providers::http::status_failure(response.status().as_u16())
            }));
        }
        let is_json = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
        if !is_json {
            return Err(fail(Failure::Protocol));
        }
        let request_id = response
            .headers()
            .get("request-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.chars().take(128).collect());
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| fail(Failure::ResponseInterrupted))?
        {
            if bytes.len().saturating_add(chunk.len()) > 2 * 1024 * 1024 {
                return Err(fail(Failure::RequestTooLarge));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| fail(Failure::Protocol))?;
        parse(&value, request_id).map_err(fail)
    };
    tokio::select! {
        biased;
        _=cancellation.cancelled()=>Err(ProviderAttemptError::Cancelled{output_started:false}),
        result=tokio::time::timeout(Duration::from_millis(timeout_ms),work)=>result.unwrap_or_else(|_|Err(fail(Failure::Timeout))),
    }
}

fn parse(value: &Value, request_id: Option<String>) -> Result<Completion, Failure> {
    if value["type"] != "message"
        || value["role"] != "assistant"
        || value["stop_reason"] != "end_turn"
    {
        return Err(Failure::Protocol);
    }
    let blocks = value["content"].as_array().ok_or(Failure::Protocol)?;
    let mut content = String::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => content.push_str(block["text"].as_str().ok_or(Failure::Protocol)?),
            Some("thinking" | "redacted_thinking") => {}
            // Native tool execution belongs to a future adapter contract. It is
            // never passed to another runtime or published as answer text.
            _ => return Err(Failure::Protocol),
        }
    }
    if content.trim().is_empty() || content.len() > 64 * 1024 {
        return Err(Failure::Protocol);
    }
    let usage = json!({"inputTokens":value["usage"]["input_tokens"].as_u64(),"outputTokens":value["usage"]["output_tokens"].as_u64(),"cacheReadTokens":value["usage"]["cache_read_input_tokens"].as_u64()});
    Ok(Completion {
        content,
        request_id,
        model: value["model"]
            .as_str()
            .map(|v| v.chars().take(128).collect()),
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_finished_public_text_is_adoptable() {
        let mut value = json!({"type":"message","role":"assistant","model":"m","stop_reason":"end_turn","content":[{"type":"thinking","thinking":"private"},{"type":"text","text":"answer"}],"usage":{"input_tokens":12}});
        let result = parse(&value, None).unwrap();
        assert_eq!(result.content, "answer");
        assert_eq!(result.usage["inputTokens"], 12);
        assert!(result.usage["outputTokens"].is_null());
        value["stop_reason"] = json!("max_tokens");
        assert!(parse(&value, None).is_err());
        value["stop_reason"] = json!("end_turn");
        value["content"][0] = json!({"type":"tool_use","name":"execute"});
        assert!(parse(&value, None).is_err());
    }
    #[tokio::test]
    async fn preserves_system_history_headers_and_rejects_redirect_without_resending() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let app = axum::Router::new().route(
            "/v1/messages",
            axum::routing::post(
                move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
                    let observed = observed.clone();
                    async move {
                        observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        assert_eq!(headers["x-api-key"], "fixture-key");
                        assert_eq!(headers["anthropic-version"], "2023-06-01");
                        assert_eq!(body["system"], "required context");
                        assert_eq!(body["messages"][0]["content"], "prior");
                        assert_eq!(body["messages"][1]["content"], "current");
                        assert_eq!(body["stream"], false);
                        (
                            axum::http::StatusCode::TEMPORARY_REDIRECT,
                            [("location", "/should-not-receive-a-key")],
                        )
                    }
                },
            ),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = run(
            &format!("http://{address}/v1"),
            Some("fixture-key"),
            "m",
            "required context",
            &[("assistant".into(), "prior".into())],
            "current",
            1000,
            4096,
            Arc::default(),
        )
        .await;
        assert!(matches!(
            result,
            Err(ProviderAttemptError::Failed {
                kind: Failure::Contract,
                ..
            })
        ));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        server.abort();
    }
}
