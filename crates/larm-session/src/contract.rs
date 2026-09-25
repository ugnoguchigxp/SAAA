use serde_json::Value;
use std::{collections::HashMap, time::Duration};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

pub const BASE_PROVIDERS: [(&str, &str); 4] = [
    ("tts", "openai.audio-speech.v1"),
    ("asr", "openai.audio-transcriptions.v1"),
    ("llm", "openai.chat-completions.v1"),
    ("embedding", "larm.embedding.v1"),
];
pub const BACKCHANNEL: (&str, &str) = ("backchannel", "openai.chat-completions.v1");
pub const DEFAULT_SELECTOR: &str = "SAAA";
/// Profile ids SAAA shipped as defaults before LARM offered selectors. Stored values resolve to `SAAA`.
pub const LEGACY_PROFILE_IDS: [&str; 3] = [
    "saaa-conversation-ornith15",
    "saaa-conversation-gemma4",
    "saaa-qwen38",
];

pub fn required_providers() -> Vec<&'static str> {
    BASE_PROVIDERS
        .iter()
        .map(|(name, _)| *name)
        .chain(std::iter::once(BACKCHANNEL.0))
        .collect()
}

pub(crate) fn verify_against_catalog(
    snapshot: &Snapshot,
    catalog: &crate::catalog::CatalogProfile,
) -> Result<(), &'static str> {
    for (name, claimed) in &snapshot.providers {
        let declared = catalog
            .provider(name)
            .ok_or("larm_catalog_claim_mismatch")?;
        if declared.model != claimed.model || declared.protocol != claimed.protocol {
            return Err("larm_catalog_claim_mismatch");
        }
        if matches!(name.as_str(), "llm" | "backchannel")
            && declared.context_window != claimed.context_window
        {
            return Err("larm_catalog_claim_mismatch");
        }
    }
    Ok(())
}

pub(crate) fn accepted_provider(name: &str) -> Option<&'static str> {
    BASE_PROVIDERS
        .iter()
        .chain(std::iter::once(&BACKCHANNEL))
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, protocol)| *protocol)
}
pub(crate) fn expected_endpoint(name: &str) -> Option<&'static str> {
    match name {
        "llm" | "backchannel" => Some("/v1/chat/completions"),
        "asr" => Some("/v1/audio/transcriptions"),
        "tts" => Some("/v1/audio/speech"),
        "embedding" => Some("/v1/embed"),
        _ => None,
    }
}

pub(crate) fn validate_created(
    value: &Value,
    selector: &str,
    required: &[&str],
    catalog: Option<&crate::catalog::CatalogProfile>,
) -> Result<(), &'static str> {
    if value["profile"] != selector || string(value, "agentProfile").is_err() {
        return Err("larm_invalid_contract");
    }
    let status = string(value, "status")?;
    if !matches!(status, "ready" | "pending" | "probing") {
        return Err("larm_startup_terminal");
    }
    let raw = value["providers"]
        .as_array()
        .ok_or("larm_invalid_contract")?;
    if raw.len() != required.len() {
        return Err("larm_missing_provider");
    }
    let mut seen = std::collections::HashSet::new();
    for provider in raw {
        let name = string(provider, "name")?;
        if !required.contains(&name) || !seen.insert(name) {
            return Err("larm_invalid_provider");
        }
        if provider["protocol"] != accepted_provider(name).ok_or("larm_invalid_provider")?
            || provider["endpoint"] != expected_endpoint(name).ok_or("larm_invalid_provider")?
            || string(provider, "model").is_err()
            || (status == "ready"
                && (provider["readiness"] != "ready" || provider["claimable"] != true))
        {
            return Err("larm_invalid_provider");
        }
        if let Some(catalog) = catalog {
            let declared = catalog
                .provider(name)
                .ok_or("larm_catalog_claim_mismatch")?;
            if provider["model"] != declared.model
                || provider["protocol"] != declared.protocol
                || declared.endpoint != expected_endpoint(name).ok_or("larm_invalid_provider")?
            {
                return Err("larm_catalog_claim_mismatch");
            }
        }
    }
    if let Some(catalog) = catalog {
        if value["agentProfile"] != catalog.id {
            return Err("larm_catalog_claim_mismatch");
        }
    }
    let expected_service = match selector {
        "SAAA-w-Image" => Some((
            "image",
            "media.image.generate",
            "larm.image-generation.v1",
            "/v1/images/generations",
        )),
        "SAAA-w-music" => Some((
            "music",
            "media.music.generate",
            "larm.music-generation.v1",
            "/v1/music/generations",
        )),
        _ => None,
    };
    let services = value["services"]
        .as_array()
        .ok_or("larm_invalid_contract")?;
    if services.len() != usize::from(expected_service.is_some()) {
        return Err("larm_invalid_service");
    }
    if let Some((name, capability, protocol, endpoint)) = expected_service {
        let service = &services[0];
        if service["name"] != name
            || service["capability"] != capability
            || service["protocol"] != protocol
            || service["endpoint"] != endpoint
            || string(service, "model").is_err()
        {
            return Err("larm_invalid_service");
        }
        if let Some(catalog) = catalog {
            let declared = catalog
                .services
                .iter()
                .find(|entry| entry.name == name)
                .ok_or("larm_invalid_service")?;
            if service["model"] != declared.model {
                return Err("larm_invalid_service");
            }
        }
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextWindow {
    pub max_tokens: u64,
    pub output_reserve_tokens: u64,
    pub safety_margin_tokens: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capacity {
    pub max_concurrent_requests: usize,
    pub active_requests: usize,
    pub max_queued_requests: usize,
    pub queue_depth: usize,
    pub queue_timeout_ms: u64,
    pub retry_after_ms: u64,
    pub completion_guaranteed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbeddingSpace {
    pub dimension: usize,
}
impl ContextWindow {
    pub fn max_input_tokens(self) -> u64 {
        self.max_tokens - self.output_reserve_tokens - self.safety_margin_tokens
    }
}
// Deliberately not Debug or Serialize: credentials never enter logs or IPC.
pub struct Provider {
    pub base_url: url::Url,
    pub model: String,
    pub protocol: String,
    pub voice: Option<String>,
    pub context_window: Option<ContextWindow>,
    pub embedding_space: Option<EmbeddingSpace>,
    token: Zeroizing<String>,
    pub(crate) health_url: url::Url,
    pub(crate) max_age: Duration,
    pub(crate) checked_at: Mutex<Option<std::time::Instant>>,
    pub(crate) capacity: tokio::sync::OnceCell<Capacity>,
    pub(crate) limiter: tokio::sync::OnceCell<std::sync::Arc<tokio::sync::Semaphore>>,
}
impl Provider {
    pub fn token(&self) -> &str {
        &self.token
    }
    pub fn endpoint(&self, operation: &str) -> Result<url::Url, &'static str> {
        let mut base = self.base_url.clone();
        base.set_path(&format!("{}/", base.path().trim_end_matches('/')));
        base.join(operation).map_err(|_| "larm_invalid_endpoint")
    }
}
pub(crate) struct Snapshot {
    pub context_subject: Option<String>,
    pub allocation_id: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub providers: HashMap<String, Provider>,
}
pub(crate) fn expiry(value: &Value) -> Result<chrono::DateTime<chrono::Utc>, &'static str> {
    let date = chrono::DateTime::parse_from_rfc3339(string(value, "expiresAt")?)
        .map_err(|_| "larm_invalid_expiry")?
        .with_timezone(&chrono::Utc);
    if date <= chrono::Utc::now() {
        return Err("larm_expired");
    }
    Ok(date)
}
pub(crate) fn string<'a>(v: &'a Value, key: &str) -> Result<&'a str, &'static str> {
    v[key]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or("larm_invalid_contract")
}
pub fn local_url(url: &url::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unique_local(),
            Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".local"),
            _ => false,
        }
}
fn endpoint(value: &str) -> Result<url::Url, &'static str> {
    let url = url::Url::parse(value).map_err(|_| "larm_invalid_endpoint")?;
    if !local_url(&url) {
        return Err("larm_nonlocal_endpoint");
    }
    Ok(url)
}
pub(crate) fn context_window(raw: &Value) -> Result<ContextWindow, &'static str> {
    let value = raw
        .get("contextWindow")
        .ok_or("larm_missing_context_window")?;
    let window = ContextWindow {
        max_tokens: value["maxTokens"]
            .as_u64()
            .ok_or("larm_invalid_context_window")?,
        output_reserve_tokens: value["outputReserveTokens"]
            .as_u64()
            .ok_or("larm_invalid_context_window")?,
        safety_margin_tokens: value["safetyMarginTokens"]
            .as_u64()
            .ok_or("larm_invalid_context_window")?,
    };
    if window.max_tokens == 0
        || window.output_reserve_tokens == 0
        || window.safety_margin_tokens == 0
        || window
            .output_reserve_tokens
            .checked_add(window.safety_margin_tokens)
            .is_none_or(|reserved| reserved >= window.max_tokens)
    {
        return Err("larm_invalid_context_window");
    }
    Ok(window)
}
fn embedding_space(raw: &Value) -> Result<EmbeddingSpace, &'static str> {
    let value = raw
        .get("embeddingSpace")
        .ok_or("larm_missing_embedding_space")?;
    let dimension = value["dimension"]
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| (1..=65_536).contains(value))
        .ok_or("larm_invalid_embedding_space")?;
    Ok(EmbeddingSpace { dimension })
}
pub(crate) fn parse(value: Value, id: &str, required: &[&str]) -> Result<Snapshot, &'static str> {
    if value["id"] != id || value["status"] != "ready" {
        return Err("larm_invalid_claim");
    }
    let expires_at = expiry(&value)?;
    let allocation_id = string(&value, "allocationId")?.to_string();
    let mut providers = HashMap::new();
    for raw in value["providers"].as_array().ok_or("larm_invalid_claim")? {
        let name = string(raw, "name")?;
        let Some(protocol) = accepted_provider(name) else {
            continue;
        };
        if providers.contains_key(name) || raw["protocol"] != protocol {
            return Err("larm_invalid_provider");
        }
        let token = string(&raw["credential"], "token")?;
        if reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).is_err() {
            return Err("larm_invalid_credential");
        }
        let max_age = raw["health"]["maxAgeMs"]
            .as_u64()
            .filter(|v| *v > 0 && *v <= 600_000)
            .ok_or("larm_invalid_health")?;
        let context_window = if name == "llm" || name == "backchannel" {
            Some(context_window(raw)?)
        } else {
            None
        };
        let embedding_space = if name == "embedding" {
            Some(embedding_space(raw)?)
        } else {
            None
        };
        let fields = &raw["configuration"]["fields"];
        let url_field = if name == "embedding" { "daemonURL" } else { "baseURL" };
        if fields[url_field] != raw["baseUrl"] || fields["model"] != raw["model"] {
            return Err("larm_invalid_provider_configuration");
        }
        providers.insert(
            name.to_string(),
            Provider {
                base_url: endpoint(string(raw, "baseUrl")?)?,
                model: string(raw, "model")?.to_string(),
                protocol: protocol.to_string(),
                voice: fields
                    .get("voice")
                    .map(|_| string(fields, "voice").map(str::to_string))
                    .transpose()?,
                context_window,
                embedding_space,
                token: Zeroizing::new(token.to_string()),
                health_url: endpoint(string(&raw["health"], "url")?)?,
                max_age: Duration::from_millis(max_age),
                checked_at: Mutex::new(None),
                capacity: tokio::sync::OnceCell::new(),
                limiter: tokio::sync::OnceCell::new(),
            },
        );
    }
    if required.iter().any(|name| !providers.contains_key(*name)) {
        return Err("larm_missing_provider");
    }
    Ok(Snapshot {
        context_subject: if value["contextControl"]["contractVersion"] == "larm-personal-state.v1" {
            let subject = string(&value["contextControl"], "subjectDigest")?;
            if subject.len() != 64
                || !subject
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err("larm_invalid_context_subject");
            }
            Some(subject.to_string())
        } else {
            None
        },
        allocation_id,
        expires_at,
        providers,
    })
}
