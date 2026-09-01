use futures_util::StreamExt;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, LOCATION, RETRY_AFTER},
    Method, StatusCode,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{env, time::Duration};
use url::Url;

use super::validate::{connection_claim_url, validate_claim};
use super::{
    contract_error, ConnectionClaim, ConnectionIdentity, DynamicLanError, ErrorEnvelope, ErrorKind,
    JsonResponse, ProviderDescriptor, ProviderHealth, API_TOKEN_ENV, MAX_RETRY_AFTER_SECONDS,
    RELEASE_TIMEOUT, REQUEST_TIMEOUT, RESPONSE_LIMIT,
};
use crate::RunCancellation;
use zeroize::Zeroizing;

pub(crate) async fn claim_and_probe(
    client: &reqwest::Client,
    control_base: &Url,
    control_credential: Option<&HeaderValue>,
    identity: &ConnectionIdentity,
    audience: &str,
    control_is_loopback: bool,
    cancellation: &RunCancellation,
) -> Result<ProviderDescriptor, DynamicLanError> {
    let claim = send_json_response::<ConnectionClaim>(
        client,
        Method::POST,
        connection_claim_url(control_base, &identity.id)?,
        control_credential,
        None,
        Some(&json!({ "format": "openai-provider-v1" })),
        cancellation,
    )
    .await?;
    if claim.status != StatusCode::OK {
        return Err(contract_error(()));
    }
    let descriptor = validate_claim(claim.value, identity, audience, control_is_loopback)?;
    probe_provider_health(client, &descriptor, cancellation).await?;
    Ok(descriptor)
}

pub(crate) async fn probe_provider_health(
    client: &reqwest::Client,
    descriptor: &ProviderDescriptor,
    cancellation: &RunCancellation,
) -> Result<(), DynamicLanError> {
    let credential = descriptor
        .credential
        .as_ref()
        .filter(|credential| credential.r#type == "bearer")
        .map(|credential| provider_credential(&credential.token))
        .transpose()?;
    let health = send_json_response::<ProviderHealth>(
        client,
        Method::GET,
        Url::parse(&descriptor.health.url).map_err(contract_error)?,
        credential.as_ref(),
        None,
        None,
        cancellation,
    )
    .await?;
    if health.status != StatusCode::OK || !health.value.ready || !health.value.accepting_requests {
        return Err(DynamicLanError::new(
            ErrorKind::Unavailable,
            "The dynamic LAN provider did not pass semantic readiness checks.",
        ));
    }
    Ok(())
}

pub(crate) async fn cancellable_sleep(
    duration: Duration,
    cancellation: &RunCancellation,
) -> Result<(), DynamicLanError> {
    tokio::select! {
        _ = cancellation.cancelled() => Err(DynamicLanError::new(ErrorKind::Cancelled, "The dynamic LAN provider connection was cancelled.")),
        _ = tokio::time::sleep(duration) => Ok(()),
    }
}

pub(crate) async fn send_json_response<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    method: Method,
    url: Url,
    credential: Option<&HeaderValue>,
    extra_header: Option<(&str, &str)>,
    body: Option<&Value>,
    cancellation: &RunCancellation,
) -> Result<JsonResponse<T>, DynamicLanError> {
    let mut request = client.request(method, url).timeout(REQUEST_TIMEOUT);
    if let Some(credential) = credential {
        request = request.header(AUTHORIZATION, credential.clone());
    }
    if let Some((name, value)) = extra_header {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(DynamicLanError::new(ErrorKind::Cancelled, "The dynamic LAN provider connection was cancelled.")),
        response = request.send() => response.map_err(classify_transport)?,
    };
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let success_metadata = status.is_success().then(|| {
        Ok::<_, DynamicLanError>((
            parse_retry_after(response.headers().get(RETRY_AFTER))?,
            bounded_header(response.headers().get(LOCATION), 512)?,
            bounded_header(response.headers().get("x-larm-config-revision"), 128)?,
        ))
    });
    let body = read_limited(response, cancellation).await?;
    if !status.is_success() {
        let code = serde_json::from_slice::<ErrorEnvelope>(&body)
            .ok()
            .map(|envelope| envelope.error.code)
            .unwrap_or_default();
        return Err(classify_status(status, &code));
    }
    let (retry_after, location, config_revision) =
        success_metadata.ok_or_else(|| contract_error(()))??;
    if !is_json_content_type(&content_type) {
        return Err(contract_error(()));
    }
    let value = serde_json::from_slice(&body).map_err(|_| contract_error(()))?;
    Ok(JsonResponse {
        value,
        status,
        retry_after,
        location,
        config_revision,
    })
}

pub(crate) fn is_json_content_type(value: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|media_type| media_type.trim() == "application/json")
}

pub(crate) fn parse_retry_after(
    value: Option<&HeaderValue>,
) -> Result<Option<Duration>, DynamicLanError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let seconds = value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| (1..=MAX_RETRY_AFTER_SECONDS).contains(seconds))
        .ok_or_else(|| contract_error(()))?;
    Ok(Some(Duration::from_secs(seconds)))
}

pub(crate) fn bounded_header(
    value: Option<&HeaderValue>,
    max_len: usize,
) -> Result<Option<String>, DynamicLanError> {
    value
        .map(|value| {
            let value = value.to_str().map_err(contract_error)?;
            if value.is_empty() || value.len() > max_len || value.chars().any(char::is_control) {
                return Err(contract_error(()));
            }
            Ok(value.to_string())
        })
        .transpose()
}

pub(crate) async fn release_connection(
    client: &reqwest::Client,
    url: &Url,
    credential: Option<&HeaderValue>,
) -> Result<(), DynamicLanError> {
    let mut request = client.delete(url.clone()).timeout(RELEASE_TIMEOUT);
    if let Some(credential) = credential {
        request = request.header(AUTHORIZATION, credential.clone());
    }
    let response = request.send().await.map_err(classify_transport)?;
    if response.status() == StatusCode::NO_CONTENT {
        Ok(())
    } else {
        Err(classify_status(response.status(), ""))
    }
}

pub(crate) async fn error_after_release(
    mut error: DynamicLanError,
    client: &reqwest::Client,
    url: &Url,
    credential: Option<&HeaderValue>,
) -> DynamicLanError {
    if let Err(release_error) = release_connection(client, url, credential).await {
        error.release_failure = Some(release_error.kind);
    }
    error
}

pub(crate) async fn read_limited(
    response: reqwest::Response,
    cancellation: &RunCancellation,
) -> Result<Vec<u8>, DynamicLanError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err(DynamicLanError::new(ErrorKind::Cancelled, "The dynamic LAN provider connection was cancelled.")),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        let chunk = chunk.map_err(classify_transport)?;
        if body.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            return Err(contract_error(()));
        }
        body.extend_from_slice(&chunk);
    }
}

pub(crate) fn control_credential() -> Result<Option<HeaderValue>, DynamicLanError> {
    let token = match env::var(API_TOKEN_ENV) {
        Ok(token) => Zeroizing::new(token),
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(env::VarError::NotUnicode(_)) => {
            return Err(DynamicLanError::new(
                ErrorKind::Authentication,
                "LARM_API_TOKEN is invalid.",
            ));
        }
    };
    provider_credential(token.as_str())
        .map(Some)
        .map_err(|_| DynamicLanError::new(ErrorKind::Authentication, "LARM_API_TOKEN is invalid."))
}

pub(crate) fn provider_credential(token: &str) -> Result<HeaderValue, DynamicLanError> {
    if token.is_empty()
        || token.len() > 4_096
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(DynamicLanError::new(
            ErrorKind::Authentication,
            "The dynamic LAN provider credential is invalid.",
        ));
    }
    let bearer = Zeroizing::new(format!("Bearer {token}"));
    let mut value = HeaderValue::from_str(bearer.as_str()).map_err(|_| {
        DynamicLanError::new(
            ErrorKind::Authentication,
            "The dynamic LAN provider credential is invalid.",
        )
    })?;
    value.set_sensitive(true);
    Ok(value)
}

pub(crate) fn classify_transport(error: reqwest::Error) -> DynamicLanError {
    if error.is_timeout() {
        DynamicLanError::new(
            ErrorKind::Timeout,
            "The dynamic_lan configuration API request timed out.",
        )
    } else {
        DynamicLanError::new(
            ErrorKind::Network,
            "Could not reach the dynamic_lan configuration API.",
        )
    }
}

pub(crate) fn classify_status(status: StatusCode, code: &str) -> DynamicLanError {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return DynamicLanError::new(
            ErrorKind::Authentication,
            "dynamic_lan rejected the connection authorization.",
        );
    }
    if matches!(status, StatusCode::NOT_FOUND | StatusCode::GONE) {
        return DynamicLanError::new(
            ErrorKind::StaleConnection,
            "The dynamic LAN provider connection is no longer active.",
        );
    }
    if status == StatusCode::CONFLICT {
        return classify_api_error(code);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return DynamicLanError::new(
            ErrorKind::Capacity,
            "dynamic_lan cannot allocate the requested provider right now.",
        );
    }
    if status == StatusCode::SERVICE_UNAVAILABLE {
        return DynamicLanError::new(
            ErrorKind::Unavailable,
            "The dynamic LAN provider service is not ready.",
        );
    }
    if status.is_server_error() {
        return DynamicLanError::new(
            ErrorKind::Upstream,
            "dynamic_lan could not resolve the provider connection.",
        );
    }
    classify_api_error(code)
}

pub(crate) fn classify_api_error(code: &str) -> DynamicLanError {
    match code {
        "capacity_exhausted" | "admission_denied" | "provider_busy" => DynamicLanError::new(
            ErrorKind::Capacity,
            "dynamic_lan cannot allocate the requested provider right now.",
        ),
        "connection_auth_not_configured" | "unauthorized" | "forbidden" => DynamicLanError::new(
            ErrorKind::Authentication,
            "dynamic_lan rejected the connection authorization.",
        ),
        "provider_semantic_not_ready" | "connection_not_ready" => DynamicLanError::new(
            ErrorKind::Unavailable,
            "The dynamic LAN provider did not pass semantic readiness checks.",
        ),
        "connection_inactive"
        | "connection_expired"
        | "connection_released"
        | "connection_boot_epoch_mismatch"
        | "connection_not_found" => DynamicLanError::new(
            ErrorKind::StaleConnection,
            "The dynamic LAN provider connection is no longer active.",
        ),
        _ => contract_error(()),
    }
}
