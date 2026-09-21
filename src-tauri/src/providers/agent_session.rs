use futures_util::StreamExt;
use reqwest::{Client, RequestBuilder, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use url::Url;

use crate::ipc_contract::ConversationMessage;
use crate::AgentSessionProviderSettings;

use super::stream::{
    CleanupOutcome, ModelStreamContext, ProviderAttemptOutcome, ProviderFailureKind,
};

mod creation;
mod sse;

pub(crate) fn initial_input_reserve(
    state: &crate::AppState,
    input: &crate::StartTurnInput,
) -> Result<usize, String> {
    sse::initial_input_reserve(state, input)
}

const MAX_HTTP_BODY_BYTES: usize = 1_048_576;

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelDescriptor>,
}

#[derive(Debug, Deserialize)]
struct ModelDescriptor {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SessionResponse {
    id: String,
    #[serde(default)]
    events_url: Option<String>,
}

enum SessionTransport {
    ServerSentEvents(Url),
}

pub(crate) async fn probe_agent_session_provider(
    provider: &AgentSessionProviderSettings,
) -> Result<String, String> {
    let client = provider_client(provider, Duration::from_secs(10))?;
    let api_key = provider_api_key(provider)?;
    verify_model(&client, provider, api_key.as_deref().map(String::as_str)).await?;
    let session = create_session(&client, provider, api_key.as_deref().map(String::as_str))
        .await
        .map_err(|kind| kind.public_message().as_str().to_string())?;
    let transport_result = session_transport(provider, &session).map_err(|_| {
        "Agent Session creation did not advertise a supported event stream URL".to_string()
    });
    let transport_probe = match transport_result.as_ref() {
        Ok(SessionTransport::ServerSentEvents(url)) => {
            sse::probe_event_stream(&client, url, api_key.as_deref().map(String::as_str)).await
        }
        Err(error) => Err(error.clone()),
    };
    let release_result = release_session(
        &client,
        provider,
        &session.id,
        api_key.as_deref().map(String::as_str),
    )
    .await
    .map_err(|kind| kind.public_message().as_str().to_string());
    transport_probe?;
    release_result?;
    let transport = match transport_result.expect("successful transport probe has a transport") {
        SessionTransport::ServerSentEvents(_) => "SSE",
    };
    Ok(format!(
        "Model {} is listed and session creation advertised an {transport} stream",
        provider.model,
    ))
}

pub(crate) async fn stream_agent_session_provider(
    provider: &AgentSessionProviderSettings,
    history: &[ConversationMessage],
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    let started = std::time::Instant::now();
    if context.cancellation.is_cancelled() {
        return cancelled(false);
    }
    let client = match provider_client(provider, Duration::from_millis(timeout_ms)) {
        Ok(client) => client,
        Err(_) => return failed(ProviderFailureKind::Internal, false),
    };
    let api_key = match provider_api_key(provider) {
        Ok(api_key) => api_key,
        Err(_) => return failed(ProviderFailureKind::Authentication, false),
    };
    let session = match creation::create_owned(
        client.clone(),
        provider.clone(),
        api_key.clone(),
        context.cancellation.clone(),
    )
    .await
    {
        Ok(session) => session,
        Err(ProviderFailureKind::Cancelled) => {
            return cancelled(false).with_cleanup(CleanupOutcome::Pending)
        }
        Err(kind) => return failed(kind, false),
    };
    let transport = match session_transport(provider, &session) {
        Ok(transport) => transport,
        Err(kind) => {
            let release = release_session_with_retry(
                &client,
                provider,
                &session.id,
                api_key.as_deref().map(String::as_str),
            )
            .await;
            return apply_release(failed(kind, false), release);
        }
    };
    let attempt = match transport {
        SessionTransport::ServerSentEvents(events_url) => {
            sse::run_agent_session_sse(
                &client,
                provider,
                &session,
                events_url,
                history,
                timeout_ms
                    .saturating_sub(started.elapsed().as_millis() as u64)
                    .max(1),
                api_key.as_deref().map(String::as_str),
                context,
            )
            .await
        }
    };
    let release = release_session_with_retry(
        &client,
        provider,
        &session.id,
        api_key.as_deref().map(String::as_str),
    )
    .await;
    apply_release(attempt, release)
}
fn apply_release(
    attempt: ProviderAttemptOutcome,
    release: Result<(), ProviderFailureKind>,
) -> ProviderAttemptOutcome {
    match release {
        Ok(()) => attempt.with_cleanup(CleanupOutcome::Released),
        Err(kind) => attempt.with_cleanup(CleanupOutcome::ReleaseFailed {
            kind: match kind {
                ProviderFailureKind::Authentication => "authentication",
                ProviderFailureKind::Network => "network",
                ProviderFailureKind::Timeout => "timeout",
                ProviderFailureKind::Capacity
                | ProviderFailureKind::Upstream
                | ProviderFailureKind::Unavailable => "upstream",
                ProviderFailureKind::Contract
                | ProviderFailureKind::Protocol
                | ProviderFailureKind::RequiredContextOverflow
                | ProviderFailureKind::ContextScopeChanged
                | ProviderFailureKind::RequiredContextUnavailable => "protocol",
                _ => "internal",
            },
        }),
    }
}

async fn verify_model(
    client: &Client,
    provider: &AgentSessionProviderSettings,
    api_key: Option<&str>,
) -> Result<(), String> {
    let response = authorized(client.get(models_url(provider)?), api_key)
        .send()
        .await
        .map_err(|error| format!("Connection failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Provider returned HTTP {}", response.status()));
    }
    let models: ModelsResponse = serde_json::from_slice(&read_bounded_body(response).await?)
        .map_err(|_| "Provider returned an invalid Agent Session models response".to_string())?;
    if !models.data.iter().any(|model| model.id == provider.model) {
        return Err(format!(
            "Configured model {} is not listed by the provider",
            provider.model
        ));
    }
    Ok(())
}

fn provider_client(
    provider: &AgentSessionProviderSettings,
    timeout: Duration,
) -> Result<Client, String> {
    let mut builder = Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none());
    if provider.location == "local" {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|error| format!("Could not initialize HTTP client: {error}"))
}

fn provider_api_key(
    provider: &AgentSessionProviderSettings,
) -> Result<Option<zeroize::Zeroizing<String>>, String> {
    if provider.authentication == "none" {
        return Ok(None);
    }
    let api_key = crate::credentials::load_api_key(&provider.id)?;
    if api_key.is_none() {
        return Err("API key is not configured in macOS Keychain".to_string());
    }
    Ok(api_key)
}

fn models_url(provider: &AgentSessionProviderSettings) -> Result<Url, String> {
    operation_url(provider, &provider.models_path)
}

fn sessions_url(provider: &AgentSessionProviderSettings) -> Result<Url, ProviderFailureKind> {
    operation_url(provider, &provider.sessions_path).map_err(|_| ProviderFailureKind::Contract)
}

fn operation_url(provider: &AgentSessionProviderSettings, path: &str) -> Result<Url, String> {
    let base = Url::parse(&provider.base_url)
        .map_err(|_| "Agent Session base URL is invalid".to_string())?;
    let url = base
        .join(path)
        .map_err(|_| "Agent Session path is invalid".to_string())?;
    if url.origin() != base.origin() {
        return Err("Agent Session path changed the configured origin".to_string());
    }
    Ok(url)
}

fn runtime_id(provider: &AgentSessionProviderSettings) -> Result<String, ProviderFailureKind> {
    let url = models_url(provider).map_err(|_| ProviderFailureKind::Contract)?;
    url.query_pairs()
        .find_map(|(key, value)| {
            (key == "runtime" && !value.is_empty()).then(|| value.into_owned())
        })
        .ok_or(ProviderFailureKind::Contract)
}

fn session_transport(
    provider: &AgentSessionProviderSettings,
    session: &SessionResponse,
) -> Result<SessionTransport, ProviderFailureKind> {
    if !safe_remote_id(&session.id) {
        return Err(ProviderFailureKind::Protocol);
    }
    if let Some(path) = session.events_url.as_deref() {
        return event_stream_url(provider, path).map(SessionTransport::ServerSentEvents);
    }
    Err(ProviderFailureKind::Contract)
}

fn event_stream_url(
    provider: &AgentSessionProviderSettings,
    path: &str,
) -> Result<Url, ProviderFailureKind> {
    let base = Url::parse(&provider.base_url).map_err(|_| ProviderFailureKind::Contract)?;
    let url = base.join(path).map_err(|_| ProviderFailureKind::Contract)?;
    if url.origin() != base.origin()
        || url.scheme() != base.scheme()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ProviderFailureKind::Contract);
    }
    Ok(url)
}

fn session_operation_url(
    provider: &AgentSessionProviderSettings,
    session_id: &str,
    operation: &str,
) -> Result<Url, ProviderFailureKind> {
    if !safe_remote_id(session_id) {
        return Err(ProviderFailureKind::Protocol);
    }
    let mut url = sessions_url(provider)?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| ProviderFailureKind::Contract)?;
        segments.pop_if_empty().push(session_id).push(operation);
    }
    Ok(url)
}

fn safe_remote_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

async fn create_session(
    client: &Client,
    provider: &AgentSessionProviderSettings,
    api_key: Option<&str>,
) -> Result<SessionResponse, ProviderFailureKind> {
    let request = authorized(client.post(sessions_url(provider)?), api_key)
        .header(
            "Idempotency-Key",
            format!("saaa_{}", uuid::Uuid::new_v4().simple()),
        )
        .json(&json!({
            "runtime": runtime_id(provider)?,
            "model": provider.model,
            "approval_policy": "strict",
            "workspace": { "mode": "isolated" }
        }));
    let session: SessionResponse = read_json_response(send(request).await?).await?;
    if !safe_remote_id(&session.id) {
        return Err(ProviderFailureKind::Protocol);
    }
    Ok(session)
}

async fn release_session(
    client: &Client,
    provider: &AgentSessionProviderSettings,
    session_id: &str,
    api_key: Option<&str>,
) -> Result<(), ProviderFailureKind> {
    release_session_request(
        client,
        provider,
        session_id,
        api_key,
        &format!("saaa_{}", uuid::Uuid::new_v4().simple()),
    )
    .await
}

async fn release_session_request(
    client: &Client,
    provider: &AgentSessionProviderSettings,
    session_id: &str,
    api_key: Option<&str>,
    idempotency_key: &str,
) -> Result<(), ProviderFailureKind> {
    send(
        authorized(
            client.post(session_operation_url(provider, session_id, "release")?),
            api_key,
        )
        .header("Idempotency-Key", idempotency_key),
    )
    .await
    .map(|_| ())
}

async fn release_session_with_retry(
    client: &Client,
    provider: &AgentSessionProviderSettings,
    session_id: &str,
    api_key: Option<&str>,
) -> Result<(), ProviderFailureKind> {
    let idempotency_key = format!("saaa_{}", uuid::Uuid::new_v4().simple());
    tokio::time::timeout(Duration::from_secs(2), async {
        for attempt in 0..3 {
            match release_session_request(client, provider, session_id, api_key, &idempotency_key)
                .await
            {
                Err(
                    ProviderFailureKind::Capacity
                    | ProviderFailureKind::Upstream
                    | ProviderFailureKind::Network
                    | ProviderFailureKind::Timeout,
                ) if attempt < 2 => tokio::time::sleep(Duration::from_millis(100)).await,
                result => return result,
            }
        }
        Err(ProviderFailureKind::Internal)
    })
    .await
    .unwrap_or(Err(ProviderFailureKind::Timeout))
}

fn authorized(request: RequestBuilder, api_key: Option<&str>) -> RequestBuilder {
    match api_key {
        Some(api_key) => request.bearer_auth(api_key),
        None => request,
    }
}

async fn send(request: RequestBuilder) -> Result<reqwest::Response, ProviderFailureKind> {
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            ProviderFailureKind::Timeout
        } else {
            ProviderFailureKind::Network
        }
    })?;
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(failure_for_status(response.status()))
    }
}

async fn read_json_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, ProviderFailureKind> {
    let body = read_bounded_body(response)
        .await
        .map_err(|_| ProviderFailureKind::Protocol)?;
    serde_json::from_slice(&body).map_err(|_| ProviderFailureKind::Protocol)
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_HTTP_BODY_BYTES as u64)
    {
        return Err("Provider response exceeded the size limit".to_string());
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("Could not read provider response: {error}"))?;
        if body.len().saturating_add(chunk.len()) > MAX_HTTP_BODY_BYTES {
            return Err("Provider response exceeded the size limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn failure_for_status(status: StatusCode) -> ProviderFailureKind {
    match status.as_u16() {
        401 | 403 => ProviderFailureKind::Authentication,
        404 | 405 | 422 => ProviderFailureKind::Contract,
        408 => ProviderFailureKind::Timeout,
        409 | 429 => ProviderFailureKind::Capacity,
        500..=599 => ProviderFailureKind::Upstream,
        _ => ProviderFailureKind::Protocol,
    }
}

fn failed(kind: ProviderFailureKind, output_started: bool) -> ProviderAttemptOutcome {
    ProviderAttemptOutcome::Failed {
        kind,
        public_message: kind.public_message(),
        output_started,
        cleanup: CleanupOutcome::NotApplicable,
    }
}

fn cancelled(output_started: bool) -> ProviderAttemptOutcome {
    ProviderAttemptOutcome::Cancelled {
        output_started,
        cleanup: CleanupOutcome::NotApplicable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn provider() -> AgentSessionProviderSettings {
        AgentSessionProviderSettings {
            id: "muse".to_string(),
            enabled: true,
            label: "Muse".to_string(),
            location: "local".to_string(),
            base_url: "http://127.0.0.1:44449/".to_string(),
            model: "muse/model".to_string(),
            models_path: "/v1/agents/models?runtime=muse".to_string(),
            sessions_path: "/v1/agents/sessions".to_string(),
            authentication: "none".to_string(),
        }
    }

    #[test]
    fn release_failure_preserves_success_and_uses_a_persistable_failure_code() {
        let outcome = apply_release(
            ProviderAttemptOutcome::Completed {
                content: "answer".into(),
                cleanup: CleanupOutcome::NotApplicable,
            },
            Err(ProviderFailureKind::Capacity),
        );
        assert_eq!(
            outcome,
            ProviderAttemptOutcome::Completed {
                content: "answer".into(),
                cleanup: CleanupOutcome::ReleaseFailed { kind: "upstream" }
            }
        );
    }

    #[test]
    fn requires_same_origin_sse_even_when_legacy_transport_is_advertised() {
        let provider = provider();
        let session = |events: serde_json::Value| {
            serde_json::from_value::<SessionResponse>(json!({
                "id": "ags_valid", "events_url": events,
                "stream_url": "ws://127.0.0.1:44449/v1/llm/stream",
                "stream_protocol": "saaa.llm-stream.v1"
            }))
            .unwrap()
        };
        assert_eq!(runtime_id(&provider).as_deref(), Ok("muse"));
        assert!(matches!(
            session_transport(
                &provider,
                &session(json!("/v1/agents/sessions/ags_valid/events"))
            ),
            Ok(SessionTransport::ServerSentEvents(_))
        ));
        for events in [
            serde_json::Value::Null,
            json!("http://example.com/events"),
            json!("ws://127.0.0.1:44449/events"),
        ] {
            assert!(session_transport(&provider, &session(events)).is_err());
        }
    }
}
