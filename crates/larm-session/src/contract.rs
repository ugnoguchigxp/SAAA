use serde_json::Value;
use std::{collections::HashMap, time::Duration};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

pub const PROVIDERS: [(&str, &str); 4] = [
    ("tts", "openai.audio-speech.v1"),
    ("asr", "openai.audio-transcriptions.v1"),
    ("backchannel", "openai.chat-completions.v1"),
    ("llm", "openai.chat-completions.v1"),
];
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
fn context_window(raw: &Value) -> Result<ContextWindow, &'static str> {
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
pub(crate) fn parse(value: Value, id: &str) -> Result<Snapshot, &'static str> {
    if value["id"] != id || value["status"] != "ready" {
        return Err("larm_invalid_claim");
    }
    let expires_at = expiry(&value)?;
    let allocation_id = string(&value, "allocationId")?.to_string();
    let mut providers = HashMap::new();
    for raw in value["providers"].as_array().ok_or("larm_invalid_claim")? {
        let name = string(raw, "name")?;
        let Some((_, protocol)) = PROVIDERS.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        if providers.contains_key(name) || raw["protocol"] != *protocol {
            return Err("larm_invalid_provider");
        }
        let fields = &raw["configuration"]["fields"];
        let token = string(&raw["credential"], "token")?;
        if reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).is_err() {
            return Err("larm_invalid_credential");
        }
        let max_age = raw["health"]["maxAgeMs"]
            .as_u64()
            .filter(|v| *v > 0 && *v <= 600_000)
            .ok_or("larm_invalid_health")?;
        let context_window = if *protocol == "openai.chat-completions.v1" {
            Some(context_window(raw)?)
        } else {
            None
        };
        providers.insert(
            name.to_string(),
            Provider {
                base_url: endpoint(string(fields, "baseURL")?)?,
                model: string(fields, "model")?.to_string(),
                protocol: protocol.to_string(),
                voice: fields
                    .get("voice")
                    .map(|_| string(fields, "voice").map(str::to_string))
                    .transpose()?,
                context_window,
                token: Zeroizing::new(token.to_string()),
                health_url: endpoint(string(&raw["health"], "url")?)?,
                max_age: Duration::from_millis(max_age),
                checked_at: Mutex::new(None),
                capacity: tokio::sync::OnceCell::new(),
                limiter: tokio::sync::OnceCell::new(),
            },
        );
    }
    if PROVIDERS
        .iter()
        .any(|(name, _)| !providers.contains_key(*name))
    {
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
