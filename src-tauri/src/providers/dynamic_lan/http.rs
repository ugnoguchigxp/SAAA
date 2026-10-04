use futures_util::StreamExt;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, LOCATION, RETRY_AFTER},
    Method, StatusCode,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

use super::validate::{connection_claim_url, connection_resource_url, validate_claim};
use super::{
    contract_error, ConnectionClaim, ConnectionIdentity, DynamicLanError, ErrorEnvelope, ErrorKind,
    JsonResponse, ProviderDescriptor, ProviderHealth, MAX_RETRY_AFTER_SECONDS, RELEASE_TIMEOUT,
    REQUEST_TIMEOUT, RESPONSE_LIMIT,
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
) -> Result<(ProviderDescriptor, ProviderHealth), DynamicLanError> {
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
    let health = probe_provider_health(client, &descriptor, cancellation).await?;
    Ok((descriptor, health))
}

pub(crate) async fn probe_provider_health(
    client: &reqwest::Client,
    descriptor: &ProviderDescriptor,
    cancellation: &RunCancellation,
) -> Result<ProviderHealth, DynamicLanError> {
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
    if health.status != StatusCode::OK
        || !health.value.ready
        || !health.value.accepting_requests
        || !super::validate::valid_capacity(&health.value.capacity)
    {
        return Err(DynamicLanError::new(
            ErrorKind::Unavailable,
            "The dynamic LAN provider did not pass semantic readiness checks.",
        ));
    }
    Ok(health.value)
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
    let schema_failure_code = response_schema_failure_code(url.path());
    let is_create = method == Method::POST && url.path().ends_with("/v1/agent-connections");
    let create_base = is_create.then(|| {
        let mut base = url.clone();
        base.set_path("/");
        base
    });
    let mut request = client.request(method, url).timeout(if is_create {
        Duration::from_secs(10)
    } else {
        REQUEST_TIMEOUT
    });
    if is_create {
        request = request.header("Prefer", "wait=1");
    }
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
    let release_location = if is_create {
        bounded_header(response.headers().get(LOCATION), 512)
            .ok()
            .flatten()
    } else {
        None
    };
    let success_metadata = status.is_success().then(|| {
        Ok::<_, DynamicLanError>((
            parse_retry_after(response.headers().get(RETRY_AFTER))?,
            bounded_header(response.headers().get(LOCATION), 512)?,
            bounded_header(response.headers().get("x-larm-config-revision"), 128)?,
        ))
    });
    let body = match read_limited(response, cancellation).await {
        Ok(body) => body,
        Err(error) => {
            return Err(cleanup_create_error(
                error,
                client,
                credential,
                create_base.as_ref().filter(|_| status.is_success()),
                release_location.as_deref(),
                &[],
            )
            .await);
        }
    };
    if !status.is_success() {
        let code = serde_json::from_slice::<ErrorEnvelope>(&body)
            .ok()
            .map(|envelope| envelope.error.code)
            .unwrap_or_default();
        return Err(classify_status(status, &code));
    }
    let (retry_after, location, config_revision) = match success_metadata {
        Some(Ok(metadata)) => metadata,
        Some(Err(error)) => {
            return Err(cleanup_create_error(
                error,
                client,
                credential,
                create_base.as_ref(),
                release_location.as_deref(),
                &body,
            )
            .await);
        }
        None => return Err(contract_error(())),
    };
    if !is_json_content_type(&content_type) {
        let error = contract_error(());
        return Err(cleanup_create_error(
            error,
            client,
            credential,
            create_base.as_ref(),
            location.as_deref(),
            &body,
        )
        .await);
    }
    let value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            let error = DynamicLanError::with_code(
                ErrorKind::Contract,
                "Harness response does not match the expected schema.",
                schema_failure_code,
            );
            return Err(cleanup_create_error(
                error,
                client,
                credential,
                create_base.as_ref(),
                location.as_deref(),
                &body,
            )
            .await);
        }
    };
    Ok(JsonResponse {
        value,
        status,
        retry_after,
        location,
        config_revision,
    })
}

async fn cleanup_create_error(
    mut error: DynamicLanError,
    client: &reqwest::Client,
    credential: Option<&HeaderValue>,
    base: Option<&Url>,
    location: Option<&str>,
    body: &[u8],
) -> DynamicLanError {
    let Some(base) = base else {
        return error;
    };
    if let Some(resource) = created_connection_url(base, location, body) {
        return error_after_release(error, client, &resource, credential).await;
    }
    // A successful create may have allocated a resource, but no safe ID was returned.
    error.release_failure = Some(ErrorKind::Contract);
    error
}

fn created_connection_url(base: &Url, location: Option<&str>, body: &[u8]) -> Option<Url> {
    let body_resource = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value["id"].as_str().map(str::to_string))
        .and_then(|id| connection_resource_url(base, &id).ok());
    let location_resource = location.and_then(|location| {
        let location = base.join(location).ok()?;
        if location.origin() != base.origin()
            || location.query().is_some()
            || location.fragment().is_some()
        {
            return None;
        }
        let id = location.path().strip_prefix("/v1/agent-connections/")?;
        let resource = connection_resource_url(base, id).ok()?;
        (resource == location).then_some(resource)
    });
    match (body_resource, location_resource) {
        (Some(body), Some(location)) if body != location => None,
        (Some(body), _) => Some(body),
        (None, location) => location,
    }
}

#[cfg(test)]
mod created_connection_url_tests {
    use super::*;

    #[test]
    fn recovers_a_connection_from_location_when_the_body_is_invalid() {
        let base = Url::parse("http://127.0.0.1:9810/").unwrap();
        let resource = created_connection_url(
            &base,
            Some("/v1/agent-connections/aconn_created"),
            b"not-json",
        )
        .unwrap();
        assert_eq!(resource.path(), "/v1/agent-connections/aconn_created");
    }

    #[test]
    fn does_not_release_a_different_connection_when_ids_disagree() {
        let base = Url::parse("http://127.0.0.1:9810/").unwrap();
        let resource = created_connection_url(
            &base,
            Some("/v1/agent-connections/aconn_other"),
            br#"{"id":"aconn_created"}"#,
        );
        assert!(resource.is_none());
    }

    #[tokio::test]
    async fn unknown_create_id_reports_deferred_cleanup() {
        let base = Url::parse("http://127.0.0.1:9810/").unwrap();
        let error = cleanup_create_error(
            contract_error(()),
            &reqwest::Client::new(),
            None,
            Some(&base),
            Some("/v1/agent-connections/aconn_other"),
            br#"{"id":"aconn_created"}"#,
        )
        .await;
        assert_eq!(error.release_failure(), Some(ErrorKind::Contract));
    }
}

// Classify using our requested operation, never text supplied by the remote server.
fn response_schema_failure_code(path: &str) -> &'static str {
    if path.ends_with("/agent-profiles") {
        "harness-catalog-schema-invalid"
    } else if path.ends_with("/claim") {
        "harness-claim-schema-invalid"
    } else if path.ends_with("/health") {
        "harness-health-schema-invalid"
    } else {
        "harness-connection-schema-invalid"
    }
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

#[path = "http/errors.rs"]
mod errors;
pub(crate) use errors::*;
