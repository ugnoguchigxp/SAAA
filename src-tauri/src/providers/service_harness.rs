use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};

use crate::RunCancellation;

mod cache;
mod health;

const MAX_DESCRIPTOR_BYTES: usize = 64 * 1_024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HarnessDescriptor {
    contract_version: String,
    revision: String,
    services: Vec<ServiceDescriptor>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ServiceDescriptor {
    pub(crate) capability: String,
    pub(crate) protocol: String,
    pub(crate) base_url: String,
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) language: Option<String>,
    #[serde(default)]
    pub(crate) voice: Option<String>,
    pub(crate) health_url: String,
    #[serde(default)]
    pub(crate) streaming: Option<StreamingDescriptor>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub(crate) enum StreamingDescriptor {
    Asr(AsrStreamingDescriptor),
    Llm(LlmStreamingDescriptor),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AsrStreamingDescriptor {
    pub(crate) protocol: String,
    pub(crate) url: String,
    pub(crate) sample_rate: u32,
    pub(crate) encoding: String,
    pub(crate) packet_milliseconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LlmStreamingDescriptor {
    pub(crate) protocol: String,
    pub(crate) url: String,
    pub(crate) encoding: String,
    pub(crate) compression: String,
    pub(crate) max_concurrent_runs: u8,
    pub(crate) max_connections: u8,
    pub(crate) resume_window_ms: u64,
    pub(crate) upstream_transport: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ResolvedAsrService {
    pub(crate) batch: ServiceDescriptor,
    pub(crate) streaming: Option<AsrStreamingDescriptor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarnessResolution {
    pub(crate) state: &'static str,
    pub(crate) revision: String,
    pub(crate) services: Vec<HarnessServiceStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarnessServiceStatus {
    pub(crate) capability: &'static str,
    pub(crate) state: &'static str,
    pub(crate) protocol: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) language: Option<String>,
    pub(crate) voice: Option<String>,
    pub(crate) message: String,
}

pub(crate) async fn resolve(address: &str) -> Result<HarnessResolution, String> {
    let descriptor = load_descriptor(address, false).await?;
    Ok(resolution_from_descriptor(descriptor).await)
}

pub(crate) async fn resolve_service(
    address: &str,
    capability: &str,
) -> Result<ServiceDescriptor, String> {
    resolve_service_inner(address, capability).await
}

/// Resolves the batch ASR endpoint and, when advertised by a v2 harness, its
/// compatible native stream endpoint from the same validated descriptor.
#[allow(dead_code)]
pub(crate) async fn resolve_asr_service(address: &str) -> Result<ResolvedAsrService, String> {
    let descriptor = load_descriptor(address, true).await?;
    let service = descriptor
        .services
        .into_iter()
        .find(|service| service.capability == "asr")
        .ok_or_else(|| "Provider Harness does not advertise asr".to_string())?;
    health::probe(&service).await?;
    Ok(ResolvedAsrService {
        streaming: match service.streaming.clone() {
            Some(StreamingDescriptor::Asr(descriptor)) => Some(descriptor),
            _ => None,
        },
        batch: service,
    })
}

pub(crate) async fn resolve_service_cancellable(
    address: &str,
    capability: &str,
    cancellation: &RunCancellation,
) -> Result<ServiceDescriptor, String> {
    if cancellation.is_cancelled() {
        return Err("Cancelled by user".to_string());
    }
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err("Cancelled by user".to_string()),
        result = resolve_service_inner(address, capability) => result,
    }
}

async fn resolve_service_inner(
    address: &str,
    capability: &str,
) -> Result<ServiceDescriptor, String> {
    let descriptor = load_descriptor(address, true).await?;
    let service = descriptor
        .services
        .into_iter()
        .find(|service| service.capability == capability)
        .ok_or_else(|| format!("Provider Harness does not advertise {capability}"))?;
    health::probe(&service).await?;
    Ok(service)
}

pub(crate) fn clear_cache() {
    cache::clear();
}

async fn load_descriptor(address: &str, allow_cached: bool) -> Result<HarnessDescriptor, String> {
    let base = validate_address(address)?;
    if allow_cached {
        if let Some(descriptor) = cache::get(&base) {
            return Ok(descriptor);
        }
    }
    let descriptor = fetch_descriptor(&base).await?;
    validate_descriptor(&base, &descriptor)?;
    cache::put(&base, descriptor.clone());
    Ok(descriptor)
}

async fn fetch_descriptor(base: &url::Url) -> Result<HarnessDescriptor, String> {
    let descriptor_url = base
        .join("v1/services")
        .map_err(|_| "Could not derive the Harness descriptor URL".to_string())?;
    let client = health::client()?;
    let response = client
        .get(descriptor_url)
        .send()
        .await
        .map_err(|_| "Could not connect to the Provider Harness".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Provider Harness returned HTTP {}",
            response.status()
        ));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "Provider Harness response was interrupted".to_string())?;
        if body.len().saturating_add(chunk.len()) > MAX_DESCRIPTOR_BYTES {
            return Err("Provider Harness descriptor exceeded the size limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    let descriptor: HarnessDescriptor = serde_json::from_slice(&body)
        .map_err(|_| "Provider Harness returned an invalid descriptor".to_string())?;
    Ok(descriptor)
}

pub(crate) async fn resolve_with_legacy_llm(address: &str) -> Result<HarnessResolution, String> {
    match resolve(address).await {
        Ok(resolution) => Ok(resolution),
        Err(primary_error) => {
            let Some(host) = legacy_dynamic_lan_host(address)? else {
                return Err(primary_error);
            };
            let connection = crate::providers::dynamic_lan::DynamicLanConnection::resolve(
                &host,
                Arc::new(RunCancellation::default()),
            )
            .await
            .map_err(|_| primary_error)?;
            let model = connection.model().to_string();
            let _ = connection.release().await;
            Ok(HarnessResolution {
                state: "degraded",
                revision: "agent-connection.v1".to_string(),
                services: vec![
                    ready_status("llm", "openai.chat-completions.v1", model, None, None),
                    missing_status("asr"),
                    missing_status("tts"),
                ],
            })
        }
    }
}

pub(crate) fn legacy_dynamic_lan_host(address: &str) -> Result<Option<String>, String> {
    let base = validate_address(address)?;
    let is_legacy_address = base.scheme() == "http"
        && base.port() == Some(crate::providers::dynamic_lan::CONTROL_PORT)
        && base.path() == "/"
        && !matches!(base.host(), Some(url::Host::Ipv6(_)));
    Ok(is_legacy_address.then(|| base.host_str().unwrap_or_default().to_string()))
}

fn validate_address(address: &str) -> Result<url::Url, String> {
    if address.is_empty() || address.len() > 2_048 {
        return Err("Harness address must contain a valid HTTP(S) URL".to_string());
    }
    let mut url = url::Url::parse(address).map_err(|_| "Harness address is invalid".to_string())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.scheme(), "http" | "https")
    {
        return Err("Harness address must not contain credentials, query, or fragment".to_string());
    }
    if url.scheme() == "http" && !is_private_harness_host(&url) {
        return Err("Public Harness addresses must use HTTPS".to_string());
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

fn is_private_harness_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => {
            host == "localhost" || host.ends_with(".local") || !host.contains('.')
        }
        Some(url::Host::Ipv4(address)) => address.is_loopback() || address.is_private(),
        Some(url::Host::Ipv6(address)) => address.is_loopback() || address.is_unique_local(),
        None => false,
    }
}

fn validate_descriptor(base: &url::Url, descriptor: &HarnessDescriptor) -> Result<(), String> {
    if !matches!(
        descriptor.contract_version.as_str(),
        "saaa-service-harness.v1" | "saaa-service-harness.v2" | "saaa-service-harness.v3"
    ) || descriptor.revision.is_empty()
        || descriptor.revision.trim() != descriptor.revision
        || descriptor.revision.len() > 160
        || descriptor.revision.chars().any(char::is_control)
        || descriptor.services.len() > 3
    {
        return Err("Provider Harness contract version or revision is invalid".to_string());
    }
    let mut capabilities = HashSet::new();
    for service in &descriptor.services {
        let expected_protocol = match service.capability.as_str() {
            "llm" if descriptor.contract_version == "saaa-service-harness.v3" => {
                "saaa.llm-stream.v1"
            }
            "llm" => "openai.chat-completions.v1",
            "asr" => "openai.audio-transcriptions.v1",
            "tts" => "openai.audio-speech.v1",
            _ => return Err("Provider Harness returned an unknown capability".to_string()),
        };
        if !capabilities.insert(service.capability.as_str())
            || service.protocol != expected_protocol
            || service.model.trim().is_empty()
            || service.model.trim() != service.model
            || service.model.chars().count() > 160
            || service.model.chars().any(char::is_control)
            || service
                .language
                .as_deref()
                .is_some_and(|language| language != "auto")
            || service.voice.as_deref().is_some_and(|voice| {
                voice.trim().is_empty()
                    || voice.trim() != voice
                    || voice.chars().count() > 160
                    || voice.chars().any(char::is_control)
            })
            || (service.capability == "tts" && service.voice.is_none())
            || (service.capability == "llm"
                && (service.language.is_some() || service.voice.is_some()))
            || (service.capability == "asr" && service.voice.is_some())
            || (service.capability == "tts" && service.language.is_some())
            || (descriptor.contract_version == "saaa-service-harness.v1"
                && service.streaming.is_some())
            || (service.capability == "llm"
                && descriptor.contract_version != "saaa-service-harness.v3"
                && service.streaming.is_some())
            || (service.capability == "tts" && service.streaming.is_some())
            || (service.capability == "asr"
                && service
                    .streaming
                    .as_ref()
                    .is_some_and(|streaming| !matches!(streaming, StreamingDescriptor::Asr(_))))
            || (descriptor.contract_version == "saaa-service-harness.v3"
                && service.capability == "llm"
                && service
                    .streaming
                    .as_ref()
                    .is_none_or(|streaming| !matches!(streaming, StreamingDescriptor::Llm(_))))
        {
            return Err("Provider Harness returned an invalid service descriptor".to_string());
        }
        for candidate in [&service.base_url, &service.health_url] {
            if candidate.len() > 2_048 {
                return Err("Provider Harness service URL is too long".to_string());
            }
            let url = url::Url::parse(candidate)
                .map_err(|_| "Provider Harness returned an invalid service URL".to_string())?;
            if url.host_str() != base.host_str()
                || (base.scheme() == "https" && url.scheme() != "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || !matches!(url.scheme(), "http" | "https")
            {
                return Err("Provider Harness service URLs must use the configured host without credentials".to_string());
            }
        }
        if let Some(streaming) = &service.streaming {
            match streaming {
                StreamingDescriptor::Asr(streaming) => {
                    validate_asr_streaming_descriptor(base, streaming)?
                }
                StreamingDescriptor::Llm(streaming) => {
                    validate_llm_streaming_descriptor(base, streaming)?
                }
            }
        }
    }
    Ok(())
}

fn validate_asr_streaming_descriptor(
    base: &url::Url,
    streaming: &AsrStreamingDescriptor,
) -> Result<(), String> {
    if streaming.protocol != "saaa.asr-stream.v1"
        || streaming.sample_rate != 16_000
        || streaming.encoding != "pcm_s16le"
        || streaming.packet_milliseconds != 100
        || streaming.url.len() > 2_048
    {
        return Err("Provider Harness returned an invalid ASR streaming descriptor".to_string());
    }
    let url = url::Url::parse(&streaming.url)
        .map_err(|_| "Provider Harness returned an invalid ASR streaming URL".to_string())?;
    let expected_scheme = if base.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    if url.scheme() != expected_scheme
        || url.host_str() != base.host_str()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Provider Harness streaming URL must use the configured host without credentials"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_llm_streaming_descriptor(
    base: &url::Url,
    streaming: &LlmStreamingDescriptor,
) -> Result<(), String> {
    if streaming.protocol != "saaa.llm-stream.v1"
        || streaming.encoding != "json-control+binary-delta-v1"
        || streaming.compression != "none"
        || !(1..=8).contains(&streaming.max_concurrent_runs)
        || streaming.max_connections != streaming.max_concurrent_runs
        || streaming.resume_window_ms < 120_000
        || !matches!(
            streaming.upstream_transport.as_str(),
            "native" | "websocket"
        )
        || streaming.url.len() > 2_048
    {
        return Err("Provider Harness returned an invalid LLM streaming descriptor".to_string());
    }
    validate_stream_url(base, &streaming.url, "LLM")
}

fn validate_stream_url(base: &url::Url, value: &str, capability: &str) -> Result<(), String> {
    let url = url::Url::parse(value)
        .map_err(|_| format!("Provider Harness returned an invalid {capability} streaming URL"))?;
    let expected_scheme = if base.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    if url.scheme() != expected_scheme
        || url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Provider Harness streaming URL must use the configured host without credentials"
                .to_string(),
        );
    }
    Ok(())
}

async fn resolution_from_descriptor(descriptor: HarnessDescriptor) -> HarnessResolution {
    let mut services = Vec::new();
    for capability in ["llm", "asr", "tts"] {
        if let Some(service) = descriptor
            .services
            .iter()
            .find(|item| item.capability == capability)
        {
            match health::probe(service).await {
                Ok(()) => services.push(ready_status(
                    capability,
                    &service.protocol,
                    service.model.clone(),
                    service.language.clone(),
                    service.voice.clone(),
                )),
                Err(error) => services.push(unavailable_status(capability, error)),
            }
        } else {
            services.push(missing_status(capability));
        }
    }
    let state = if services.iter().all(|service| service.state == "ready") {
        "ready"
    } else {
        "degraded"
    };
    HarnessResolution {
        state,
        revision: descriptor.revision,
        services,
    }
}

fn ready_status(
    capability: &str,
    protocol: &str,
    model: String,
    language: Option<String>,
    voice: Option<String>,
) -> HarnessServiceStatus {
    HarnessServiceStatus {
        capability: match capability {
            "llm" => "llm",
            "asr" => "asr",
            _ => "tts",
        },
        state: "ready",
        protocol: Some(protocol.to_string()),
        model: Some(model),
        language,
        voice,
        message: "Resolved by Provider Harness".to_string(),
    }
}

fn missing_status(capability: &str) -> HarnessServiceStatus {
    unavailable_status(
        capability,
        "Capability is not advertised by this Harness".to_string(),
    )
}

fn unavailable_status(capability: &str, message: String) -> HarnessServiceStatus {
    HarnessServiceStatus {
        capability: match capability {
            "llm" => "llm",
            "asr" => "asr",
            _ => "tts",
        },
        state: "unavailable",
        protocol: None,
        model: None,
        language: None,
        voice: None,
        message,
    }
}

#[cfg(test)]
mod tests;
