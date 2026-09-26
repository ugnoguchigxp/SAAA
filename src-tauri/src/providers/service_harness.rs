use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
#[serde(rename_all = "camelCase")]
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HarnessResolution {
    pub(crate) state: &'static str,
    pub(crate) revision: String,
    pub(crate) services: Vec<HarnessServiceStatus>,
    #[serde(skip)]
    pub(crate) diagnosis_timings: Option<DiagnosisTimings>,
}

#[derive(Debug, Clone)]
pub(crate) struct DiagnosisTimings {
    pub(crate) connect_ms: u64,
    pub(crate) embedding_ms: u64,
    pub(crate) release_ms: u64,
    pub(crate) provider_ms: Vec<(&'static str, u64)>,
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
        .ok_or_else(|| {
            crate::providers::stream::ProviderFailureKind::Unavailable
                .public_message()
                .as_str()
                .to_string()
        })?;
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

pub(crate) async fn resolve_with_legacy_llm(
    address: &str,
    stored_profile: Option<&str>,
) -> Result<HarnessResolution, String> {
    resolve_with_legacy_llm_limit(
        address,
        stored_profile,
        std::time::Duration::from_secs(300),
        false,
    )
    .await
}

pub(crate) async fn resolve_for_diagnosis(
    address: &str,
    stored_profile: Option<&str>,
) -> Result<HarnessResolution, String> {
    resolve_with_legacy_llm_limit(
        address,
        stored_profile,
        std::time::Duration::from_secs(45),
        true,
    )
    .await
}

async fn resolve_with_legacy_llm_limit(
    address: &str,
    stored_profile: Option<&str>,
    connect_limit: std::time::Duration,
    probe_all: bool,
) -> Result<HarnessResolution, String> {
    let Some(_) = legacy_dynamic_lan_host(address)? else {
        return resolve(address).await;
    };
    let credential = crate::providers::dynamic_lan::credential::load()
        .map_err(|error| error.code().to_string())?;
    let preference = crate::providers::larm_resources::profile::preference(stored_profile);
    let (_cancel, receiver) = tokio::sync::watch::channel(false);
    let connect_started = std::time::Instant::now();
    let session = match tokio::time::timeout(
        connect_limit,
        saaa_larm_session::Session::connect_with_profile_credential_and_key(
            address,
            preference,
            credential.token().to_string(),
            format!("saaa-diagnosis-{}", uuid::Uuid::new_v4().simple()),
            receiver,
        ),
    )
    .await
    {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            if let Some(cleanup) = &error.cleanup {
                let _ = cleanup.close().await;
            }
            return Err(error.to_string());
        }
        Err(_) => return Err("larm_diagnosis_connection_timeout".into()),
    };
    let connect_ms = connect_started.elapsed().as_millis() as u64;
    let summary = session.provider_summary().await;
    let mut services = ["llm", "backchannel", "asr", "tts", "embedding"]
        .into_iter()
        .map(
            |capability| match summary.iter().find(|provider| provider.name == capability) {
                Some(provider) => HarnessServiceStatus {
                    capability,
                    state: "ready",
                    protocol: None,
                    model: Some(provider.model.clone()),
                    language: None,
                    voice: None,
                    message: format!("{} · {}", provider.model, provider.endpoint),
                },
                None => missing_status(capability),
            },
        )
        .collect::<Vec<_>>();
    let capabilities: &[&str] = if probe_all {
        &["llm", "backchannel", "asr", "tts", "embedding"]
    } else {
        &["embedding"]
    };
    let probes = capabilities
        .iter()
        .copied()
        .map(|name| probe_claimed_provider(std::sync::Arc::clone(&session), name));
    let results = futures_util::future::join_all(probes).await;
    let mut embedding_ms = 0;
    let mut provider_ms = Vec::new();
    for (capability, result, elapsed) in results {
        provider_ms.push((capability, elapsed));
        if capability == "embedding" {
            embedding_ms = elapsed;
        }
        if let Some(service) = services
            .iter_mut()
            .find(|item| item.capability == capability)
        {
            service.state = if result.is_ok() {
                "ready"
            } else if probe_all {
                "unavailable"
            } else {
                "degraded"
            };
            service.message = match result {
                Ok(message) => message,
                Err(error) => error,
            };
        }
    }
    let release_started = std::time::Instant::now();
    if session.close().await.is_err() {
        return Err("larm_diagnosis_release_failed".into());
    }
    let release_ms = release_started.elapsed().as_millis() as u64;
    Ok(HarnessResolution {
        state: "ready",
        revision: "agent-connection.v1".to_string(),
        services,
        diagnosis_timings: Some(DiagnosisTimings {
            connect_ms,
            embedding_ms,
            release_ms,
            provider_ms,
        }),
    })
}

async fn probe_claimed_provider(
    session: std::sync::Arc<saaa_larm_session::Session>,
    capability: &'static str,
) -> (&'static str, Result<String, String>, u64) {
    let started = std::time::Instant::now();
    let timeout = if capability == "embedding" { 5 } else { 10 };
    let result = tokio::time::timeout(std::time::Duration::from_secs(timeout), async {
        if capability == "embedding" {
            return session
                .embed_query(&["診断".to_string()])
                .await
                .map_err(str::to_string)
                .and_then(|vectors| {
                    vectors
                        .first()
                        .filter(|vector| !vector.is_empty())
                        .map(|_| "Embedding returned a vector".to_string())
                        .ok_or_else(|| "Embedding returned no vector".to_string())
                });
        }
        let lease = session.acquire(capability).await.map_err(str::to_string)?;
        let provider = lease.provider();
        let endpoint = provider.base_url.to_string();
        let model = provider.model.clone();
        let token = provider.token();
        match capability {
            "llm" | "backchannel" => {
                let settings = crate::OpenAiCompatibleProviderSettings {
                    request_options: None,
                    id: format!("diagnosis-{capability}"),
                    enabled: true,
                    label: capability.into(),
                    location: "local".into(),
                    endpoint,
                    model,
                    authentication: "api-key".into(),
                };
                crate::providers::openai_compatible::probe_model_provider_with_api_key(
                    &settings,
                    Some(token),
                )
                .await
            }
            "asr" => {
                let settings = crate::CloudAsrProviderSettings {
                    id: "diagnosis-asr".into(),
                    enabled: true,
                    label: "ASR".into(),
                    location: "local".into(),
                    endpoint,
                    model,
                    language: "auto".into(),
                    authentication: "api-key".into(),
                };
                let samples: Vec<f32> = (0..1600)
                    .map(|index| {
                        ((index as f32 * 440.0 * std::f32::consts::TAU / 16000.0).sin()) * 0.1
                    })
                    .collect();
                match crate::voice::cloud_asr::transcribe_with_api_key(
                    &settings,
                    &samples,
                    16_000,
                    10_000,
                    std::sync::Arc::default(),
                    Some(token),
                )
                .await
                {
                    Ok(_) => Ok("ASR accepted a fixed WAV upload".into()),
                    Err(error) if error.starts_with("ASR_NO_SPEECH:") => {
                        Ok("ASR accepted a fixed WAV upload (no speech)".into())
                    }
                    Err(error) => Err(error),
                }
            }
            "tts" => {
                let settings = crate::CloudTtsProviderSettings {
                    id: "diagnosis-tts".into(),
                    enabled: true,
                    label: "TTS".into(),
                    location: "local".into(),
                    endpoint,
                    model,
                    voice: provider.voice.clone().unwrap_or_default(),
                    response_format: "wav".into(),
                    authentication: "api-key".into(),
                    style: None,
                    speed: None,
                    pitch_scale: None,
                    intonation_scale: None,
                };
                let response = crate::voice::cloud_tts::request_audio_with_api_key(
                    &settings,
                    "Connectivity check",
                    10_000,
                    std::sync::Arc::default(),
                    Some(token),
                )
                .await?;
                crate::voice::cloud_tts::validate_audio_headers(
                    &response,
                    &settings.response_format,
                )?;
                let mut audio = response.bytes_stream();
                let mut bytes = 0_usize;
                while let Some(chunk) = audio.next().await {
                    bytes = bytes.saturating_add(
                        chunk
                            .map_err(|_| "TTS audio response was interrupted".to_string())?
                            .len(),
                    );
                    if bytes > 2_000_000 {
                        return Err("TTS audio exceeded the diagnosis size limit".into());
                    }
                }
                if bytes == 0 {
                    Err("TTS returned empty audio".into())
                } else {
                    Ok("TTS generated audio".into())
                }
            }
            _ => Err("Unsupported diagnosis capability".into()),
        }
    })
    .await
    .unwrap_or_else(|_| Err(format!("Provider probe timed out after {timeout}s")));
    (capability, result, started.elapsed().as_millis() as u64)
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
    if url.scheme() == "http" && !crate::providers::dynamic_lan::url_is_local(&url) {
        return Err("Public Harness addresses must use HTTPS".to_string());
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
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
        diagnosis_timings: None,
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
            "backchannel" => "backchannel",
            "asr" => "asr",
            "embedding" => "embedding",
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
            "backchannel" => "backchannel",
            "asr" => "asr",
            "embedding" => "embedding",
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
