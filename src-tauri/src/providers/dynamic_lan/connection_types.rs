pub(crate) const CONTROL_PORT: u16 = 9810;
pub(crate) const AUDIENCE: &str = "saaa-desktop";
const API_TOKEN_ENV: &str = "LARM_API_TOKEN";
const CLIENT_ID: &str = "saaa-desktop";
const CONNECTION_TTL_SECONDS: u32 = 900;
const READY_TIMEOUT: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_secs(1);
const MAX_RETRY_AFTER_SECONDS: u64 = 30;
const RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_LIFETIME_MARGIN: Duration = Duration::from_secs(30);
pub(crate) const MAX_REQUEST_TIMEOUT_MS: u64 =
    ((CONNECTION_TTL_SECONDS as u64) - REQUEST_LIFETIME_MARGIN.as_secs()) * 1_000 - 1;
const CLOCK_SKEW_TOLERANCE_SECONDS: i64 = 60;
const RESPONSE_LIMIT: usize = 256 * 1024;
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ErrorKind {
    Authentication,
    Contract,
    Capacity,
    Unavailable,
    Upstream,
    Network,
    Timeout,
    StaleConnection,
    Cancelled,
    Internal,
}
#[derive(Debug)]
pub(crate) struct DynamicLanError {
    pub(crate) kind: ErrorKind,
    message: &'static str,
    code: Option<&'static str>,
    release_failure: Option<ErrorKind>,
}
impl DynamicLanError {
    pub(crate) fn new(kind: ErrorKind, message: &'static str) -> Self {
        Self {
            kind,
            message,
            code: None,
            release_failure: None,
        }
    }

    pub(crate) fn with_code(kind: ErrorKind, message: &'static str, code: &'static str) -> Self {
        Self {
            kind,
            message,
            code: Some(code),
            release_failure: None,
        }
    }

    pub(crate) fn public_message(&self) -> &'static str {
        self.code.unwrap_or(self.message)
    }

    pub(crate) fn release_failure(&self) -> Option<ErrorKind> {
        self.release_failure
    }

    #[cfg(test)]
    pub(crate) fn code(&self) -> Option<&'static str> {
        self.code
    }
}
#[derive(Debug, Clone, Eq, PartialEq)]
struct SelectedLlmProfile {
    selector: String,
    catalog_revision: Option<String>,
    catalog_models: Option<std::collections::BTreeMap<String, String>>,
    id: String,
    capability: String,
    model: String,
    protocol: String,
    context_window: ProviderContextWindow,
    compare_catalog: bool,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionState {
    id: String,
    allocation_id: String,
    boot_epoch: String,
    catalog_revision: String,
    profile: String,
    agent_profile: String,
    profile_revision: String,
    audience: String,
    audience_revision: String,
    status: String,
    providers: Vec<ConnectionStateProvider>,
    services: Vec<serde_json::Value>,
    created_at: String,
    expires_at: String,
    #[serde(default)]
    error: Option<ApiError>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionStateProvider {
    name: String,
    capability: String,
    route: String,
    protocol: String,
    endpoint: String,
    model: String,
    readiness: String,
    claimable: bool,
}
#[derive(Debug, Deserialize)]
struct ApiError {
    code: String,
}
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ApiError,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionClaim {
    id: String,
    allocation_id: String,
    status: String,
    audience: String,
    providers: Vec<ProviderDescriptor>,
    expires_at: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderDescriptor {
    name: String,
    capability: String,
    api_style: String,
    protocol: String,
    scheme: String,
    host: String,
    port: u16,
    base_url: String,
    model: String,
    #[serde(default)]
    context_window: Option<ProviderContextWindow>,
    health: ProviderHealthDescriptor,
    #[serde(default)]
    credential: Option<ProviderCredential>,
    configuration: ProviderConfiguration,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderCredential {
    r#type: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    expires_at: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderHealthDescriptor {
    url: String,
    kind: String,
    max_age_ms: u64,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderHealth {
    ready: bool,
    accepting_requests: bool,
    capacity: ProviderCapacity,
}
#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ProviderCapacity {
    max_concurrent_requests: u32,
    active_requests: u32,
    max_queued_requests: u32,
    queue_depth: u32,
    queue_timeout_ms: u64,
    retry_after_ms: u64,
    completion_guaranteed: bool,
}
fn capacity_gate(capacity: &ProviderCapacity) -> Arc<tokio::sync::Semaphore> {
    Arc::new(tokio::sync::Semaphore::new(
        capacity.max_concurrent_requests as usize,
    ))
}
#[cfg(test)]
fn test_context_window() -> ProviderContextWindow {
    ProviderContextWindow {
        max_tokens: 32_768,
        output_reserve_tokens: 4_096,
        safety_margin_tokens: 1_024,
    }
}
#[cfg(test)]
fn test_provider_capacity() -> ProviderCapacity {
    ProviderCapacity {
        max_concurrent_requests: 1,
        active_requests: 0,
        max_queued_requests: 2,
        queue_depth: 0,
        queue_timeout_ms: 5_000,
        retry_after_ms: 100,
        completion_guaranteed: false,
    }
}
#[derive(Debug, Deserialize)]
struct ProviderConfiguration {
    kind: String,
    fields: ProviderConfigurationFields,
    #[serde(default, rename = "secretFields")]
    secret_fields: Option<ProviderSecretFields>,
}
#[derive(Debug, Deserialize)]
struct ProviderConfigurationFields {
    #[serde(rename = "baseURL")]
    base_url: String,
    model: String,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderSecretFields {
    #[serde(default)]
    api_key: Option<String>,
}
struct JsonResponse<T> {
    value: T,
    status: StatusCode,
    retry_after: Option<Duration>,
    location: Option<String>,
    config_revision: Option<String>,
}
#[derive(Clone)]
struct ConnectionIdentity {
    id: String,
    allocation_id: String,
    boot_epoch: String,
    catalog_revision: String,
    profile_revision: String,
    audience_revision: String,
    profile: SelectedLlmProfile,
    created_at: chrono::DateTime<chrono::FixedOffset>,
    expires_at: chrono::DateTime<chrono::FixedOffset>,
}
pub(crate) struct DynamicLanConnection {
    client: reqwest::Client,
    control_base: Url,
    control_credential: Option<HeaderValue>,
    identity: ConnectionIdentity,
    audience: String,
    endpoint: String,
    model: String,
    api_key: Option<Zeroizing<String>>,
    context_window: ProviderContextWindow,
    capacity: ProviderCapacity,
    capacity_gate: Arc<tokio::sync::Semaphore>,
    prior_release_failure: Option<ErrorKind>,
}
