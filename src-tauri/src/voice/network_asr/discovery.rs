use serde::Deserialize;
use url::{Host, Url};

#[cfg(test)]
use super::client;
use super::{bounded_response, request_error_message, PROVIDER_ID};
use crate::{NetworkAsrResolution, RunCancellation};
use std::time::Duration;

const ASR_PORT: u16 = 8081;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Deserialize)]
struct HealthResponse {
    status: String,
    model: String,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelDescriptor>,
}

#[derive(Debug, Deserialize)]
struct ModelDescriptor {
    id: String,
}

#[cfg(test)]
pub(super) async fn resolve_at(base_url: &str) -> Result<NetworkAsrResolution, String> {
    let cancellation = RunCancellation::default();
    resolve_at_with_client(&client()?, base_url, &cancellation).await
}

pub(super) async fn resolve_at_with_client(
    client: &reqwest::Client,
    base_url: &str,
    cancellation: &RunCancellation,
) -> Result<NetworkAsrResolution, String> {
    let base_url = validate_base_url(base_url)?;
    let models_response = tokio::select! {
        _ = cancellation.cancelled() => return Err("Transcription cancelled".to_string()),
        response = client
            .get(format!("{base_url}/v1/models"))
            .timeout(DISCOVERY_TIMEOUT)
            .send() => response.map_err(|error| request_error_message(&error))?,
    };
    let models_status = models_response.status();
    let models_body = bounded_response(models_response, cancellation).await?;
    if !models_status.is_success() {
        return Err(format!(
            "LAN ASR settings query returned HTTP {}",
            models_status.as_u16()
        ));
    }
    let models: ModelsResponse = serde_json::from_slice(&models_body)
        .map_err(|_| "LAN ASR returned an invalid model settings response".to_string())?;

    let health_response = tokio::select! {
        _ = cancellation.cancelled() => return Err("Transcription cancelled".to_string()),
        response = client
            .get(format!("{base_url}/health"))
            .timeout(DISCOVERY_TIMEOUT)
            .send() => response.map_err(|error| request_error_message(&error))?,
    };
    let health_status = health_response.status();
    let health_body = bounded_response(health_response, cancellation).await?;
    if !health_status.is_success() {
        return Err(format!(
            "LAN ASR health check returned HTTP {}",
            health_status.as_u16()
        ));
    }
    let health: HealthResponse = serde_json::from_slice(&health_body)
        .map_err(|_| "LAN ASR returned an invalid health response".to_string())?;
    if health.status != "ok"
        || health.model.trim().is_empty()
        || health.model.chars().count() > 160
        || health.model.chars().any(char::is_control)
        || health.model != super::MODEL_ID
        || !models.data.iter().any(|model| model.id == health.model)
    {
        return Err(
            "LAN ASR settings and health responses do not identify the same ready model"
                .to_string(),
        );
    }
    Ok(NetworkAsrResolution {
        provider_id: PROVIDER_ID.to_string(),
        endpoint: base_url,
        model: health.model,
    })
}

pub(crate) fn base_url_from_host(host: &str) -> Result<String, String> {
    let mut url = crate::providers::dynamic_lan::control_base_url(host)
        .map_err(|error| error.public_message().to_string())?;
    url.set_port(Some(ASR_PORT)).map_err(|_| {
        "Could not derive the LAN ASR endpoint from the configured host".to_string()
    })?;
    validate_base_url(url.as_str())
}

pub(super) fn ensure_selected_model(
    resolution: &NetworkAsrResolution,
    selected_model: &str,
) -> Result<(), String> {
    if resolution.model != selected_model {
        return Err(format!(
            "LAN ASR currently reports model {}, but Voice settings select {}. Resolve the ASR settings again.",
            resolution.model, selected_model
        ));
    }
    Ok(())
}

pub(super) fn validate_base_url(value: &str) -> Result<String, String> {
    let url = Url::parse(value.trim())
        .map_err(|_| "LAN ASR endpoint must be a valid private-network HTTP origin".to_string())?;
    let private_host = match url.host() {
        Some(Host::Ipv4(address)) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        Some(Host::Ipv6(address)) => {
            address.is_loopback() || address.is_unique_local() || address.is_unicast_link_local()
        }
        Some(Host::Domain(domain)) => !domain.contains('.') || domain.ends_with(".local"),
        None => false,
    };
    if url.scheme() != "http"
        || !private_host
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err("LAN ASR endpoint must be a private-network HTTP origin without credentials, path, query, or fragment".to_string());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}
