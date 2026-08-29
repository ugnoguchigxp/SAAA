use reqwest::{header::HeaderValue, Method, StatusCode};
use serde::Deserialize;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::RunCancellation;

mod http;
mod validate;

pub(crate) use http::control_credential_available;
use http::*;
pub(crate) use validate::control_base_url;
use validate::*;

pub(crate) const CONTROL_PORT: u16 = 9810;
pub(crate) const AGENT_PROFILE: &str = "deep-reasoning-35b";
pub(crate) const AUDIENCE: &str = "saaa-desktop";
const PROFILE_CAPABILITY: &str = "llm.reasoning";
const API_TOKEN_ENV: &str = "LARM_API_TOKEN";
const CLIENT_ID: &str = "saaa-desktop";
const CONNECTION_TTL_SECONDS: u32 = 300;
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
    release_failure: Option<ErrorKind>,
}

impl DynamicLanError {
    pub(crate) fn new(kind: ErrorKind, message: &'static str) -> Self {
        Self {
            kind,
            message,
            release_failure: None,
        }
    }

    pub(crate) fn public_message(&self) -> &'static str {
        self.message
    }

    pub(crate) fn release_failure(&self) -> Option<ErrorKind> {
        self.release_failure
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentProfiles {
    contract_version: String,
    profiles: Vec<AgentProfile>,
    audiences: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AgentProfile {
    id: String,
    providers: Vec<ProfileProvider>,
}

#[derive(Debug, Deserialize)]
struct ProfileProvider {
    name: String,
    capability: String,
    protocol: String,
    model: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionState {
    id: String,
    allocation_id: String,
    boot_epoch: String,
    catalog_revision: String,
    agent_profile: String,
    profile_revision: String,
    audience: String,
    audience_revision: String,
    status: String,
    providers: Vec<ConnectionStateProvider>,
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
    public_model: String,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionClaim {
    id: String,
    allocation_id: String,
    status: String,
    audience: String,
    providers: Vec<ProviderDescriptor>,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
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
    health: ProviderHealthDescriptor,
    credential: ProviderCredential,
    configuration: ProviderConfiguration,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderCredential {
    r#type: String,
    token: String,
    expires_at: String,
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
}

#[derive(Debug, Deserialize)]
struct ProviderConfiguration {
    kind: String,
    fields: ProviderConfigurationFields,
    #[serde(rename = "secretFields")]
    secret_fields: ProviderSecretFields,
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
    api_key: String,
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
    created_at: chrono::DateTime<chrono::FixedOffset>,
    expires_at: chrono::DateTime<chrono::FixedOffset>,
}

pub(crate) struct DynamicLanConnection {
    client: reqwest::Client,
    control_base: Url,
    control_credential: HeaderValue,
    identity: ConnectionIdentity,
    audience: String,
    endpoint: String,
    model: String,
    api_key: Zeroizing<String>,
    prior_release_failure: Option<ErrorKind>,
}

impl DynamicLanConnection {
    pub(crate) async fn resolve(
        host: &str,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, DynamicLanError> {
        Self::resolve_at(control_base_url(host)?, cancellation).await
    }

    async fn resolve_at(
        control_base: Url,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, DynamicLanError> {
        let control_credential = control_credential()?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| {
                DynamicLanError::new(
                    ErrorKind::Internal,
                    "Could not initialize the dynamic_lan discovery client.",
                )
            })?;
        let mut prior_release_failure = None;
        for attempt in 0..2 {
            match Self::resolve_once(
                client.clone(),
                control_base.clone(),
                control_credential.clone(),
                cancellation.clone(),
            )
            .await
            {
                Ok(mut connection) => {
                    connection.prior_release_failure =
                        prior_release_failure.or(connection.prior_release_failure);
                    return Ok(connection);
                }
                Err(error) if error.kind == ErrorKind::StaleConnection && attempt == 0 => {
                    prior_release_failure = prior_release_failure.or(error.release_failure);
                }
                Err(mut error) => {
                    error.release_failure = prior_release_failure.or(error.release_failure);
                    return Err(error);
                }
            }
        }
        let mut error = DynamicLanError::new(
            ErrorKind::StaleConnection,
            "The dynamic_lan daemon restarted while resolving the provider connection.",
        );
        error.release_failure = prior_release_failure;
        Err(error)
    }

    async fn resolve_once(
        client: reqwest::Client,
        control_base: Url,
        control_credential: HeaderValue,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, DynamicLanError> {
        let control_is_loopback = url_is_loopback(&control_base);
        let profiles = send_json_response::<AgentProfiles>(
            &client,
            Method::GET,
            control_base
                .join("v1/agent-profiles")
                .map_err(contract_error)?,
            &control_credential,
            None,
            None,
            &cancellation,
        )
        .await?;
        validate_config_revision(profiles.config_revision.as_deref())?;
        validate_profiles(&profiles.value)?;
        let audience = select_audience(&profiles.value.audiences)?.to_string();

        let idempotency_key = format!("saaa-{}", Uuid::new_v4().simple());
        let create_body = json!({
            "agentProfile": AGENT_PROFILE,
            "audience": audience.as_str(),
            "client": CLIENT_ID,
            "ttlSeconds": CONNECTION_TTL_SECONDS,
            "allowFallback": false,
            "deploymentPolicy": "existing-only"
        });
        let create_url = control_base
            .join("v1/agent-connections")
            .map_err(contract_error)?;
        let created = send_json_response::<ConnectionState>(
            &client,
            Method::POST,
            create_url,
            &control_credential,
            Some(("idempotency-key", idempotency_key.as_str())),
            Some(&create_body),
            &cancellation,
        )
        .await?;
        if !matches!(
            created.status,
            StatusCode::OK | StatusCode::CREATED | StatusCode::ACCEPTED
        ) {
            return Err(contract_error(()));
        }
        let mut state = created.value;
        let identity = match validate_initial_state(&state, &audience) {
            Ok(identity) => identity,
            Err(error) => {
                if let Ok(url) = connection_resource_url(&control_base, &state.id) {
                    return Err(
                        error_after_release(error, &client, &url, &control_credential).await,
                    );
                }
                return Err(error);
            }
        };
        let connection_url = connection_resource_url(&control_base, &identity.id)?;
        if matches!(created.status, StatusCode::CREATED | StatusCode::ACCEPTED) {
            if let Err(error) = validate_create_location(
                created.location.as_deref(),
                &control_base,
                &connection_url,
            ) {
                return Err(error_after_release(
                    error,
                    &client,
                    &connection_url,
                    &control_credential,
                )
                .await);
            }
        }
        let mut poll_interval = created.retry_after.unwrap_or(POLL_INTERVAL);
        let ready_deadline = tokio::time::Instant::now() + READY_TIMEOUT;

        loop {
            match state.status.as_str() {
                "ready" => break,
                "pending" | "probing" => {}
                "failed" => {
                    let kind = state
                        .error
                        .as_ref()
                        .map(|error| error.code.as_str())
                        .unwrap_or_default();
                    let error = classify_api_error(kind);
                    return Err(error_after_release(
                        error,
                        &client,
                        &connection_url,
                        &control_credential,
                    )
                    .await);
                }
                "released" | "expired" => {
                    let error = DynamicLanError::new(
                        ErrorKind::StaleConnection,
                        "The dynamic LAN provider connection became inactive before it was ready.",
                    );
                    return Err(error_after_release(
                        error,
                        &client,
                        &connection_url,
                        &control_credential,
                    )
                    .await);
                }
                _ => {
                    return Err(error_after_release(
                        contract_error(()),
                        &client,
                        &connection_url,
                        &control_credential,
                    )
                    .await);
                }
            }
            let remaining = ready_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                let error = DynamicLanError::new(
                    ErrorKind::Timeout,
                    "dynamic_lan did not finish resolving the provider before the startup timeout.",
                );
                return Err(error_after_release(
                    error,
                    &client,
                    &connection_url,
                    &control_credential,
                )
                .await);
            }
            if let Err(error) = cancellable_sleep(poll_interval.min(remaining), &cancellation).await
            {
                return Err(error_after_release(
                    error,
                    &client,
                    &connection_url,
                    &control_credential,
                )
                .await);
            }
            if tokio::time::Instant::now() >= ready_deadline {
                let error = DynamicLanError::new(
                    ErrorKind::Timeout,
                    "dynamic_lan did not finish resolving the provider before the startup timeout.",
                );
                return Err(error_after_release(
                    error,
                    &client,
                    &connection_url,
                    &control_credential,
                )
                .await);
            }
            let polled = send_json_response::<ConnectionState>(
                &client,
                Method::GET,
                connection_url.clone(),
                &control_credential,
                None,
                None,
                &cancellation,
            )
            .await;
            match polled {
                Ok(next) => {
                    if let Err(error) = validate_successor_state(&next.value, &identity, &audience)
                    {
                        return Err(error_after_release(
                            error,
                            &client,
                            &connection_url,
                            &control_credential,
                        )
                        .await);
                    }
                    poll_interval = next.retry_after.unwrap_or(POLL_INTERVAL);
                    state = next.value;
                }
                Err(error) => {
                    return Err(error_after_release(
                        error,
                        &client,
                        &connection_url,
                        &control_credential,
                    )
                    .await);
                }
            }
        }

        let descriptor = match claim_and_probe(
            &client,
            &control_base,
            &control_credential,
            &identity,
            &audience,
            control_is_loopback,
            &cancellation,
        )
        .await
        {
            Ok(descriptor) => descriptor,
            Err(error) => {
                return Err(error_after_release(
                    error,
                    &client,
                    &connection_url,
                    &control_credential,
                )
                .await);
            }
        };

        Ok(Self {
            client,
            control_base,
            control_credential,
            identity,
            audience,
            endpoint: descriptor.configuration.fields.base_url,
            model: descriptor.configuration.fields.model,
            api_key: Zeroizing::new(descriptor.credential.token),
            prior_release_failure: None,
        })
    }

    pub(crate) fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub(crate) fn api_key(&self) -> &str {
        self.api_key.as_str()
    }

    pub(crate) fn prior_release_failure(&self) -> Option<ErrorKind> {
        self.prior_release_failure
    }

    pub(crate) async fn ensure_lifetime(
        &mut self,
        request_timeout: Duration,
        cancellation: Arc<RunCancellation>,
    ) -> Result<(), DynamicLanError> {
        let required = request_timeout.saturating_add(REQUEST_LIFETIME_MARGIN);
        if required >= Duration::from_secs(CONNECTION_TTL_SECONDS.into()) {
            return Err(DynamicLanError::new(
                ErrorKind::Contract,
                "The configured provider timeout exceeds the dynamic_lan connection lifetime.",
            ));
        }
        let remaining = self
            .identity
            .expires_at
            .signed_duration_since(chrono::Utc::now())
            .to_std()
            .unwrap_or_default();
        if remaining.is_zero() {
            return Err(DynamicLanError::new(
                ErrorKind::StaleConnection,
                "The dynamic LAN provider connection expired before inference started.",
            ));
        }
        if remaining >= required {
            return Ok(());
        }

        let renew_key = format!("saaa-renew-{}", Uuid::new_v4().simple());
        let renewed = send_json_response::<ConnectionState>(
            &self.client,
            Method::POST,
            connection_renew_url(&self.control_base, &self.identity.id)?,
            &self.control_credential,
            Some(("idempotency-key", renew_key.as_str())),
            Some(&json!({ "ttlSeconds": CONNECTION_TTL_SECONDS })),
            &cancellation,
        )
        .await?;
        if renewed.status != StatusCode::OK {
            return Err(contract_error(()));
        }
        let next_identity = validate_renewed_state(&renewed.value, &self.identity, &self.audience)?;
        let descriptor = claim_and_probe(
            &self.client,
            &self.control_base,
            &self.control_credential,
            &next_identity,
            &self.audience,
            url_is_loopback(&self.control_base),
            &cancellation,
        )
        .await?;
        self.identity = next_identity;
        self.endpoint = descriptor.configuration.fields.base_url;
        self.model = descriptor.configuration.fields.model;
        self.api_key = Zeroizing::new(descriptor.credential.token);
        Ok(())
    }

    pub(crate) async fn release(&self) -> Result<(), DynamicLanError> {
        let url = connection_resource_url(&self.control_base, &self.identity.id)?;
        release_connection(&self.client, &url, &self.control_credential).await
    }
}

fn contract_error<T>(_: T) -> DynamicLanError {
    DynamicLanError::new(
        ErrorKind::Contract,
        "dynamic_lan returned an incompatible agent-connection response.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::CONTENT_TYPE;
    use serde_json::Value;
    use std::env;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    const TEST_REVISION: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn test_timestamps() -> (String, String) {
        let created = chrono::Utc::now() - chrono::Duration::seconds(1);
        let expires = created + chrono::Duration::seconds(CONNECTION_TTL_SECONDS.into());
        (created.to_rfc3339(), expires.to_rfc3339())
    }

    fn connection_state_json(
        id: &str,
        status: &str,
        audience: &str,
        created_at: &str,
        expires_at: &str,
    ) -> Value {
        let readiness = if status == "ready" { "ready" } else { status };
        json!({
            "id": id,
            "allocationId": "alloc_test",
            "bootEpoch": "epoch_test",
            "catalogRevision": TEST_REVISION,
            "agentProfile": AGENT_PROFILE,
            "profileRevision": TEST_REVISION,
            "audience": audience,
            "audienceRevision": TEST_REVISION,
            "status": status,
            "providers": [{
                "name": "llm",
                "capability": PROFILE_CAPABILITY,
                "route": "llm-agent-35b",
                "protocol": "openai.chat-completions.v1",
                "publicModel": AGENT_PROFILE,
                "readiness": readiness,
                "claimable": status == "ready"
            }],
            "createdAt": created_at,
            "expiresAt": expires_at,
            "error": null
        })
    }

    fn test_identity(id: &str, created_at: &str, expires_at: &str) -> ConnectionIdentity {
        ConnectionIdentity {
            id: id.to_string(),
            allocation_id: "alloc_test".to_string(),
            boot_epoch: "epoch_test".to_string(),
            catalog_revision: TEST_REVISION.to_string(),
            profile_revision: TEST_REVISION.to_string(),
            audience_revision: TEST_REVISION.to_string(),
            created_at: chrono::DateTime::parse_from_rfc3339(created_at)
                .expect("created timestamp"),
            expires_at: chrono::DateTime::parse_from_rfc3339(expires_at).expect("expiry timestamp"),
        }
    }

    fn claim_json(host: &str, port: u16, audience: &str, expires_at: &str) -> Value {
        let base_url = format!("http://{host}:{port}/v1");
        json!({
            "id": "aconn_test",
            "allocationId": "alloc_test",
            "status": "ready",
            "audience": audience,
            "expiresAt": expires_at,
            "providers": [{
                "name": "llm",
                "capability": PROFILE_CAPABILITY,
                "apiStyle": "openai",
                "protocol": "openai.chat-completions.v1",
                "scheme": "http",
                "host": host,
                "port": port,
                "baseUrl": base_url,
                "model": AGENT_PROFILE,
                "health": {
                    "url": format!("http://{host}:{port}/v1/agent-connections/aconn_test/providers/llm/health"),
                    "kind": "semantic-inference",
                    "maxAgeMs": 10_000
                },
                "credential": {
                    "type": "bearer",
                    "token": "short-lived-provider-token",
                    "expiresAt": expires_at
                },
                "configuration": {
                    "kind": "openai-provider-v1",
                    "fields": {
                        "baseURL": base_url,
                        "model": AGENT_PROFILE
                    },
                    "secretFields": { "apiKey": "credential.token" }
                }
            }]
        })
    }

    #[test]
    fn derives_control_url_from_host_only() {
        assert_eq!(
            control_base_url("10.0.0.42")
                .expect("private host")
                .as_str(),
            "http://10.0.0.42:9810/"
        );
        assert_eq!(
            control_base_url("dynamic-lan")
                .expect("single-label host")
                .as_str(),
            "http://dynamic-lan:9810/"
        );
    }

    #[test]
    fn rejects_urls_ports_and_public_hosts() {
        for host in [
            "http://dynamic_lan",
            "dynamic_lan:8083",
            "example.com",
            "dynamic_lan/path",
            "-proxy",
            "proxy-",
            "foo..local",
            "foo-.local",
            "[::1]",
        ] {
            assert!(control_base_url(host).is_err(), "{host}");
        }
    }

    #[test]
    fn builds_connection_urls_only_from_bounded_identifier_segments() {
        let base = Url::parse("http://127.0.0.1:9810/").expect("control URL");
        assert_eq!(
            connection_resource_url(&base, "aconn_epoch_uuid")
                .expect("connection URL")
                .as_str(),
            "http://127.0.0.1:9810/v1/agent-connections/aconn_epoch_uuid"
        );
        assert_eq!(
            connection_claim_url(&base, "aconn_epoch_uuid")
                .expect("claim URL")
                .as_str(),
            "http://127.0.0.1:9810/v1/agent-connections/aconn_epoch_uuid/claim"
        );
        for id in [".", "..", "aconn/other", "aconn:other", "aconn.other"] {
            assert!(connection_resource_url(&base, id).is_err(), "{id}");
        }
    }

    #[test]
    fn requires_one_exact_deep_reasoning_profile() {
        let profile = || AgentProfile {
            id: AGENT_PROFILE.to_string(),
            providers: vec![ProfileProvider {
                name: "llm".to_string(),
                capability: PROFILE_CAPABILITY.to_string(),
                protocol: "openai.chat-completions.v1".to_string(),
                model: AGENT_PROFILE.to_string(),
            }],
        };
        let profiles = AgentProfiles {
            contract_version: "agent-connection.v1".to_string(),
            profiles: vec![profile()],
            audiences: vec!["saaa-desktop".to_string()],
        };
        assert!(validate_profiles(&profiles).is_ok());

        let duplicate = AgentProfiles {
            contract_version: profiles.contract_version.clone(),
            profiles: vec![profile(), profile()],
            audiences: profiles.audiences.clone(),
        };
        assert!(validate_profiles(&duplicate).is_err());
    }

    #[test]
    fn requires_the_fixed_saaa_desktop_audience() {
        let audiences = vec!["same-host".to_string(), "saaa-desktop".to_string()];
        assert_eq!(select_audience(&audiences).expect("audience"), AUDIENCE);
        assert!(select_audience(&["same-host".to_string()]).is_err());
        assert!(select_audience(&["unknown-network".to_string()]).is_err());
        assert!(select_audience(&[AUDIENCE.to_string(), AUDIENCE.to_string()]).is_err());
    }

    #[test]
    fn accepts_only_the_json_media_type_for_success_responses() {
        assert!(is_json_content_type("application/json"));
        assert!(is_json_content_type("application/json; charset=utf-8"));
        assert!(!is_json_content_type("application/jsonp"));
        assert!(!is_json_content_type("application/problem+json"));
    }

    #[test]
    fn accepts_a_private_http_endpoint_and_rejects_loopback_for_remote_dynamic_lan() {
        let (created_at, expires_at) = test_timestamps();
        let identity = test_identity("aconn_test", &created_at, &expires_at);
        let claim = |host: &str| {
            serde_json::from_value::<ConnectionClaim>(claim_json(
                host,
                CONTROL_PORT,
                AUDIENCE,
                &expires_at,
            ))
            .expect("claim fixture")
        };

        let descriptor = validate_claim(claim("10.0.0.42"), &identity, AUDIENCE, false)
            .expect("private HTTP descriptor is accepted");
        assert_eq!(descriptor.base_url, "http://10.0.0.42:9810/v1");
        assert!(validate_claim(claim("127.0.0.1"), &identity, AUDIENCE, false).is_err());
        let wrong_identity = test_identity("different-id", &created_at, &expires_at);
        assert!(validate_claim(claim("10.0.0.42"), &wrong_identity, AUDIENCE, false).is_err());
    }

    #[test]
    fn renewed_expiry_allows_only_the_bounded_clock_skew() {
        let created_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
        let initial_expires_at = (chrono::Utc::now() + chrono::Duration::seconds(45)).to_rfc3339();
        let expected = test_identity("aconn_test", &created_at, &initial_expires_at);
        let within_skew = (chrono::Utc::now() + chrono::Duration::seconds(330)).to_rfc3339();
        let state: ConnectionState = serde_json::from_value(connection_state_json(
            "aconn_test",
            "ready",
            AUDIENCE,
            &created_at,
            &within_skew,
        ))
        .expect("renewed state fixture");
        assert!(validate_renewed_state(&state, &expected, AUDIENCE).is_ok());

        let beyond_skew = (chrono::Utc::now() + chrono::Duration::seconds(361)).to_rfc3339();
        let state: ConnectionState = serde_json::from_value(connection_state_json(
            "aconn_test",
            "ready",
            AUDIENCE,
            &created_at,
            &beyond_skew,
        ))
        .expect("overlong renewed state fixture");
        assert!(validate_renewed_state(&state, &expected, AUDIENCE).is_err());
    }

    #[tokio::test]
    async fn error_status_is_classified_before_success_only_retry_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let _ = read_request(&mut stream);
            write_response_with_headers(
                &mut stream,
                "429 Too Many Requests",
                "application/json",
                "Retry-After: 120\r\nLocation: invalid location\r\n",
                &json!({ "error": { "code": "capacity_exhausted" } }).to_string(),
            );
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client");
        let error = match send_json_response::<Value>(
            &client,
            Method::GET,
            Url::parse(&format!("http://{address}/v1/agent-profiles")).expect("test URL"),
            &provider_credential("test-control-token").expect("test credential"),
            None,
            None,
            &RunCancellation::default(),
        )
        .await
        {
            Ok(_) => panic!("429 must fail"),
            Err(error) => error,
        };
        server.join().expect("server joins");
        assert_eq!(error.kind, ErrorKind::Capacity);
    }

    #[tokio::test]
    async fn initialization_error_preserves_a_release_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let request = read_request(&mut stream);
            assert!(request.starts_with("DELETE "));
            write_response(
                &mut stream,
                "503 Service Unavailable",
                "application/json",
                "{}",
            );
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client");
        let error = error_after_release(
            contract_error(()),
            &client,
            &Url::parse(&format!("http://{address}/v1/agent-connections/aconn_test"))
                .expect("release URL"),
            &provider_credential("test-control-token").expect("test credential"),
        )
        .await;
        server.join().expect("server joins");
        assert_eq!(error.kind, ErrorKind::Contract);
        assert_eq!(error.release_failure(), Some(ErrorKind::Unavailable));
    }

    #[tokio::test]
    async fn releases_the_original_connection_when_poll_identity_changes() {
        let _environment = super::super::larm::test_environment_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let (created_at, expires_at) = test_timestamps();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..4 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                let request = read_request(&mut stream);
                captured_tx.send(request).expect("request captured");
                match index {
                    0 => write_response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &json!({
                            "contractVersion": "agent-connection.v1",
                            "profiles": [{
                                "id": AGENT_PROFILE,
                                "providers": [{
                                    "name": "llm",
                                    "capability": PROFILE_CAPABILITY,
                                    "protocol": "openai.chat-completions.v1",
                                    "model": AGENT_PROFILE
                                }]
                            }],
                            "audiences": [AUDIENCE]
                        })
                        .to_string(),
                    ),
                    1 => write_response_with_headers(
                        &mut stream,
                        "202 Accepted",
                        "application/json",
                        "Location: /v1/agent-connections/aconn_original\r\nRetry-After: 1\r\n",
                        &connection_state_json(
                            "aconn_original",
                            "pending",
                            AUDIENCE,
                            &created_at,
                            &expires_at,
                        )
                        .to_string(),
                    ),
                    2 => write_response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &connection_state_json(
                            "aconn_changed",
                            "ready",
                            AUDIENCE,
                            &created_at,
                            &expires_at,
                        )
                        .to_string(),
                    ),
                    3 => write_response(&mut stream, "204 No Content", "", ""),
                    _ => unreachable!(),
                }
            }
        });

        let error = match DynamicLanConnection::resolve_at(
            Url::parse(&format!("http://{address}/")).expect("control URL"),
            Arc::new(RunCancellation::default()),
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("changed connection identity must be rejected"),
        };
        assert_eq!(error.kind, ErrorKind::Contract);
        server.join().expect("server joins");
        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 4);
        assert!(requests[2].starts_with("GET /v1/agent-connections/aconn_original HTTP/1.1"));
        assert!(requests[3].starts_with("DELETE /v1/agent-connections/aconn_original HTTP/1.1"));

        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }

    #[tokio::test]
    async fn resolves_claimed_openai_settings_and_releases_the_connection() {
        let _environment = super::super::larm::test_environment_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let (created_at, expires_at) = test_timestamps();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..5 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                let request = read_request(&mut stream);
                captured_tx.send(request.clone()).expect("request captured");
                let body = match index {
                    0 => json!({
                        "contractVersion": "agent-connection.v1",
                        "profiles": [{
                            "id": AGENT_PROFILE,
                            "providers": [{
                                "name": "llm",
                                "capability": PROFILE_CAPABILITY,
                                "protocol": "openai.chat-completions.v1",
                                "model": AGENT_PROFILE
                            }]
                        }],
                        "audiences": [AUDIENCE]
                    })
                    .to_string(),
                    1 => connection_state_json(
                        "aconn_test",
                        "ready",
                        AUDIENCE,
                        &created_at,
                        &expires_at,
                    )
                    .to_string(),
                    2 => claim_json("127.0.0.1", address.port(), AUDIENCE, &expires_at).to_string(),
                    3 => json!({ "ready": true, "acceptingRequests": true }).to_string(),
                    4 => String::new(),
                    _ => unreachable!(),
                };
                if index == 4 {
                    write_response(&mut stream, "204 No Content", "", "");
                } else {
                    write_response(&mut stream, "200 OK", "application/json", &body);
                }
            }
        });

        let connection = DynamicLanConnection::resolve_at(
            Url::parse(&format!("http://{address}/")).expect("control URL"),
            Arc::new(RunCancellation::default()),
        )
        .await
        .expect("connection resolves");
        assert_eq!(
            connection.endpoint(),
            format!("http://127.0.0.1:{}/v1", address.port())
        );
        assert_eq!(connection.model(), AGENT_PROFILE);
        assert_eq!(connection.api_key(), "short-lived-provider-token");
        connection.release().await.expect("connection releases");
        server.join().expect("server joins");

        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 5);
        assert!(requests[0].starts_with("GET /v1/agent-profiles HTTP/1.1"));
        assert!(requests[1].starts_with("POST /v1/agent-connections HTTP/1.1"));
        assert!(requests[1].contains("\"agentProfile\":\"deep-reasoning-35b\""));
        assert!(requests[1].contains("\"ttlSeconds\":300"));
        assert!(requests[2].starts_with("POST /v1/agent-connections/aconn_test/claim HTTP/1.1"));
        assert!(!requests[2]
            .to_ascii_lowercase()
            .contains("idempotency-key:"));
        assert!(requests[3]
            .starts_with("GET /v1/agent-connections/aconn_test/providers/llm/health HTTP/1.1"));
        assert!(requests[4].starts_with("DELETE /v1/agent-connections/aconn_test HTTP/1.1"));
        for index in [0, 1, 2, 4] {
            assert!(requests[index]
                .to_ascii_lowercase()
                .contains("authorization: bearer test-control-token"));
        }
        assert!(requests[3]
            .to_ascii_lowercase()
            .contains("authorization: bearer short-lived-provider-token"));

        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }

    #[tokio::test]
    async fn releases_when_semantic_health_is_not_ready() {
        let _environment = super::super::larm::test_environment_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let (created_at, expires_at) = test_timestamps();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..5 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                let request = read_request(&mut stream);
                captured_tx.send(request).expect("request captured");
                let body = match index {
                    0 => json!({
                        "contractVersion": "agent-connection.v1",
                        "profiles": [{
                            "id": AGENT_PROFILE,
                            "providers": [{
                                "name": "llm",
                                "capability": PROFILE_CAPABILITY,
                                "protocol": "openai.chat-completions.v1",
                                "model": AGENT_PROFILE
                            }]
                        }],
                        "audiences": [AUDIENCE]
                    })
                    .to_string(),
                    1 => connection_state_json(
                        "aconn_test",
                        "ready",
                        AUDIENCE,
                        &created_at,
                        &expires_at,
                    )
                    .to_string(),
                    2 => claim_json("127.0.0.1", address.port(), AUDIENCE, &expires_at).to_string(),
                    3 => json!({ "ready": true, "acceptingRequests": false }).to_string(),
                    4 => String::new(),
                    _ => unreachable!(),
                };
                if index == 4 {
                    write_response(&mut stream, "204 No Content", "", "");
                } else {
                    write_response(&mut stream, "200 OK", "application/json", &body);
                }
            }
        });

        let error = match DynamicLanConnection::resolve_at(
            Url::parse(&format!("http://{address}/")).expect("control URL"),
            Arc::new(RunCancellation::default()),
        )
        .await
        {
            Ok(_) => panic!("semantic health failure must reject the connection"),
            Err(error) => error,
        };
        assert_eq!(error.kind, ErrorKind::Unavailable);
        server.join().expect("server joins");
        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 5);
        assert!(requests[4].starts_with("DELETE /v1/agent-connections/aconn_test HTTP/1.1"));

        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }

    #[tokio::test]
    async fn renews_and_reclaims_before_the_request_deadline() {
        let _environment = super::super::larm::test_environment_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let created_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
        let initial_expires_at = (chrono::Utc::now() + chrono::Duration::seconds(45)).to_rfc3339();
        let renewed_expires_at = (chrono::Utc::now() + chrono::Duration::seconds(299)).to_rfc3339();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..8 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                let request = read_request(&mut stream);
                captured_tx.send(request).expect("request captured");
                let body = match index {
                    0 => json!({
                        "contractVersion": "agent-connection.v1",
                        "profiles": [{
                            "id": AGENT_PROFILE,
                            "providers": [{
                                "name": "llm",
                                "capability": PROFILE_CAPABILITY,
                                "protocol": "openai.chat-completions.v1",
                                "model": AGENT_PROFILE
                            }]
                        }],
                        "audiences": [AUDIENCE]
                    })
                    .to_string(),
                    1 => connection_state_json(
                        "aconn_test",
                        "ready",
                        AUDIENCE,
                        &created_at,
                        &initial_expires_at,
                    )
                    .to_string(),
                    2 => claim_json("127.0.0.1", address.port(), AUDIENCE, &initial_expires_at)
                        .to_string(),
                    3 | 6 => json!({ "ready": true, "acceptingRequests": true }).to_string(),
                    4 => connection_state_json(
                        "aconn_test",
                        "ready",
                        AUDIENCE,
                        &created_at,
                        &renewed_expires_at,
                    )
                    .to_string(),
                    5 => claim_json("127.0.0.1", address.port(), AUDIENCE, &renewed_expires_at)
                        .to_string(),
                    7 => String::new(),
                    _ => unreachable!(),
                };
                if index == 7 {
                    write_response(&mut stream, "204 No Content", "", "");
                } else {
                    write_response(&mut stream, "200 OK", "application/json", &body);
                }
            }
        });

        let mut connection = DynamicLanConnection::resolve_at(
            Url::parse(&format!("http://{address}/")).expect("control URL"),
            Arc::new(RunCancellation::default()),
        )
        .await
        .expect("connection resolves");
        connection
            .ensure_lifetime(
                Duration::from_secs(30),
                Arc::new(RunCancellation::default()),
            )
            .await
            .expect("connection renews and reclaims");
        connection.release().await.expect("connection releases");
        server.join().expect("server joins");

        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 8);
        assert!(requests[4].starts_with("POST /v1/agent-connections/aconn_test/renew HTTP/1.1"));
        assert!(requests[4]
            .to_ascii_lowercase()
            .contains("idempotency-key:"));
        assert!(requests[5].starts_with("POST /v1/agent-connections/aconn_test/claim HTTP/1.1"));
        assert!(!requests[5]
            .to_ascii_lowercase()
            .contains("idempotency-key:"));
        assert!(requests[6]
            .starts_with("GET /v1/agent-connections/aconn_test/providers/llm/health HTTP/1.1"));
        assert!(requests[7].starts_with("DELETE /v1/agent-connections/aconn_test HTTP/1.1"));

        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }

    #[tokio::test]
    #[ignore = "operator-only live dynamic_lan Agent Connection API canary"]
    async fn live_dynamic_lan_claim_and_chat() {
        let _environment = super::super::larm::test_environment_lock().lock().await;
        let host = env::var("SAAA_DYNAMIC_LAN_HOST").expect("SAAA_DYNAMIC_LAN_HOST is required");
        let connection = DynamicLanConnection::resolve(&host, Arc::new(RunCancellation::default()))
            .await
            .expect("live dynamic_lan connection resolves");
        let chat_url = Url::parse(connection.endpoint())
            .expect("claimed endpoint is valid")
            .join("chat/completions")
            .expect("chat URL joins");
        let response = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("live client builds")
            .post(chat_url)
            .bearer_auth(connection.api_key())
            .timeout(Duration::from_secs(60))
            .json(&json!({
                "model": connection.model(),
                "messages": [{ "role": "user", "content": "Reply with exactly: SAAA_DYNAMIC_OK" }],
                "stream": true,
                "max_tokens": 64,
                "reasoning_effort": "low"
            }))
            .send()
            .await;
        let release = connection.release().await;
        let response = response.expect("claimed provider request completes");
        let status = response.status();
        assert!(status.is_success(), "claimed provider returned {status}");
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(
            content_type.starts_with("text/event-stream"),
            "claimed provider must return SSE"
        );
        let body = read_limited(response, &RunCancellation::default())
            .await
            .expect("provider SSE reads");
        let body = std::str::from_utf8(&body).expect("provider SSE is UTF-8");
        assert!(body.lines().any(|line| line.trim() == "data: [DONE]"));
        let completion = body
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim)
            .filter(|data| *data != "[DONE]")
            .filter_map(|data| serde_json::from_str::<Value>(data).ok())
            .filter_map(|value| {
                value
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect::<String>();
        assert!(!completion.trim().is_empty());
        release.expect("live connection releases");
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected = None;
        loop {
            let read = stream.read(&mut buffer).expect("request reads");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected.is_none() {
                if let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    expected = Some(header_end + 4 + content_length);
                }
            }
            if expected.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        String::from_utf8(request).expect("request is UTF-8")
    }

    fn write_response(
        stream: &mut std::net::TcpStream,
        status: &str,
        content_type: &str,
        body: &str,
    ) {
        write_response_with_headers(stream, status, content_type, "", body);
    }

    fn write_response_with_headers(
        stream: &mut std::net::TcpStream,
        status: &str,
        content_type: &str,
        extra_headers: &str,
        body: &str,
    ) {
        let content_type = if content_type.is_empty() {
            String::new()
        } else {
            format!("Content-Type: {content_type}\r\n")
        };
        let revision = if content_type.is_empty() {
            String::new()
        } else {
            format!("x-larm-config-revision: {TEST_REVISION}\r\n")
        };
        write!(
            stream,
            "HTTP/1.1 {status}\r\n{content_type}{revision}{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("response writes");
    }
}
