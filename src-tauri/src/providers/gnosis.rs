use futures_util::StreamExt;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, LOCATION, RETRY_AFTER},
    Method, StatusCode,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{env, sync::Arc, time::Duration};
use url::{Host, Url};
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::RunCancellation;

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
pub(crate) struct GnosisError {
    pub(crate) kind: ErrorKind,
    message: &'static str,
    release_failure: Option<ErrorKind>,
}

impl GnosisError {
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

pub(crate) struct GnosisConnection {
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

impl GnosisConnection {
    pub(crate) async fn resolve(
        host: &str,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, GnosisError> {
        Self::resolve_at(control_base_url(host)?, cancellation).await
    }

    async fn resolve_at(
        control_base: Url,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, GnosisError> {
        let control_credential = control_credential()?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| {
                GnosisError::new(
                    ErrorKind::Internal,
                    "Could not initialize the gnosis discovery client.",
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
        let mut error = GnosisError::new(
            ErrorKind::StaleConnection,
            "The gnosis daemon restarted while resolving the provider connection.",
        );
        error.release_failure = prior_release_failure;
        Err(error)
    }

    async fn resolve_once(
        client: reqwest::Client,
        control_base: Url,
        control_credential: HeaderValue,
        cancellation: Arc<RunCancellation>,
    ) -> Result<Self, GnosisError> {
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
                    let error = GnosisError::new(
                        ErrorKind::StaleConnection,
                        "The gnosis provider connection became inactive before it was ready.",
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
                let error = GnosisError::new(
                    ErrorKind::Timeout,
                    "gnosis did not finish resolving the provider before the startup timeout.",
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
                let error = GnosisError::new(
                    ErrorKind::Timeout,
                    "gnosis did not finish resolving the provider before the startup timeout.",
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
    ) -> Result<(), GnosisError> {
        let required = request_timeout.saturating_add(REQUEST_LIFETIME_MARGIN);
        if required >= Duration::from_secs(CONNECTION_TTL_SECONDS.into()) {
            return Err(GnosisError::new(
                ErrorKind::Contract,
                "The configured provider timeout exceeds the gnosis connection lifetime.",
            ));
        }
        let remaining = self
            .identity
            .expires_at
            .signed_duration_since(chrono::Utc::now())
            .to_std()
            .unwrap_or_default();
        if remaining.is_zero() {
            return Err(GnosisError::new(
                ErrorKind::StaleConnection,
                "The gnosis provider connection expired before inference started.",
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

    pub(crate) async fn release(&self) -> Result<(), GnosisError> {
        let url = connection_resource_url(&self.control_base, &self.identity.id)?;
        release_connection(&self.client, &url, &self.control_credential).await
    }
}

fn validate_config_revision(revision: Option<&str>) -> Result<(), GnosisError> {
    let revision = revision.ok_or_else(|| contract_error(()))?;
    validate_revision(revision)
}

fn validate_revision(revision: &str) -> Result<(), GnosisError> {
    if revision.len() == 64 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(contract_error(()))
    }
}

fn validate_initial_state(
    state: &ConnectionState,
    expected_audience: &str,
) -> Result<ConnectionIdentity, GnosisError> {
    validate_state_shape(state, expected_audience)?;
    let created_at =
        chrono::DateTime::parse_from_rfc3339(&state.created_at).map_err(contract_error)?;
    let expires_at =
        chrono::DateTime::parse_from_rfc3339(&state.expires_at).map_err(contract_error)?;
    let lifetime = expires_at.signed_duration_since(created_at);
    if created_at > chrono::Utc::now() + chrono::Duration::seconds(60)
        || expires_at <= chrono::Utc::now()
        || lifetime <= chrono::Duration::zero()
        || lifetime > chrono::Duration::seconds(CONNECTION_TTL_SECONDS.into())
    {
        return Err(contract_error(()));
    }
    Ok(ConnectionIdentity {
        id: state.id.clone(),
        allocation_id: state.allocation_id.clone(),
        boot_epoch: state.boot_epoch.clone(),
        catalog_revision: state.catalog_revision.clone(),
        profile_revision: state.profile_revision.clone(),
        audience_revision: state.audience_revision.clone(),
        created_at,
        expires_at,
    })
}

fn validate_successor_state(
    state: &ConnectionState,
    expected: &ConnectionIdentity,
    expected_audience: &str,
) -> Result<(), GnosisError> {
    validate_state_shape(state, expected_audience)?;
    let created_at =
        chrono::DateTime::parse_from_rfc3339(&state.created_at).map_err(contract_error)?;
    let expires_at =
        chrono::DateTime::parse_from_rfc3339(&state.expires_at).map_err(contract_error)?;
    if state.boot_epoch != expected.boot_epoch {
        return Err(GnosisError::new(
            ErrorKind::StaleConnection,
            "The gnosis daemon restarted while resolving the provider connection.",
        ));
    }
    if state.id != expected.id
        || state.allocation_id != expected.allocation_id
        || state.catalog_revision != expected.catalog_revision
        || state.profile_revision != expected.profile_revision
        || state.audience_revision != expected.audience_revision
        || created_at != expected.created_at
        || expires_at != expected.expires_at
    {
        return Err(contract_error(()));
    }
    Ok(())
}

fn validate_renewed_state(
    state: &ConnectionState,
    expected: &ConnectionIdentity,
    expected_audience: &str,
) -> Result<ConnectionIdentity, GnosisError> {
    validate_state_shape(state, expected_audience)?;
    let created_at =
        chrono::DateTime::parse_from_rfc3339(&state.created_at).map_err(contract_error)?;
    let expires_at =
        chrono::DateTime::parse_from_rfc3339(&state.expires_at).map_err(contract_error)?;
    if state.boot_epoch != expected.boot_epoch {
        return Err(GnosisError::new(
            ErrorKind::StaleConnection,
            "The gnosis daemon restarted while renewing the provider connection.",
        ));
    }
    if state.status != "ready"
        || state.id != expected.id
        || state.allocation_id != expected.allocation_id
        || state.catalog_revision != expected.catalog_revision
        || state.profile_revision != expected.profile_revision
        || state.audience_revision != expected.audience_revision
        || created_at != expected.created_at
        || expires_at <= expected.expires_at
        || expires_at <= chrono::Utc::now()
        || expires_at
            > chrono::Utc::now()
                + chrono::Duration::seconds(
                    i64::from(CONNECTION_TTL_SECONDS) + CLOCK_SKEW_TOLERANCE_SECONDS,
                )
    {
        return Err(contract_error(()));
    }
    Ok(ConnectionIdentity {
        id: state.id.clone(),
        allocation_id: state.allocation_id.clone(),
        boot_epoch: state.boot_epoch.clone(),
        catalog_revision: state.catalog_revision.clone(),
        profile_revision: state.profile_revision.clone(),
        audience_revision: state.audience_revision.clone(),
        created_at,
        expires_at,
    })
}

fn validate_state_shape(
    state: &ConnectionState,
    expected_audience: &str,
) -> Result<(), GnosisError> {
    validate_connection_id(&state.id)?;
    if !valid_bounded_identifier(&state.allocation_id, 192)
        || !valid_bounded_identifier(&state.boot_epoch, 192)
        || state.agent_profile != AGENT_PROFILE
        || state.audience != expected_audience
        || state.providers.len() != 1
    {
        return Err(contract_error(()));
    }
    validate_revision(&state.catalog_revision)?;
    validate_revision(&state.profile_revision)?;
    validate_revision(&state.audience_revision)?;
    let provider = &state.providers[0];
    let terminal = matches!(state.status.as_str(), "failed" | "released" | "expired");
    if provider.name != "llm"
        || provider.capability != PROFILE_CAPABILITY
        || provider.protocol != "openai.chat-completions.v1"
        || provider.public_model != AGENT_PROFILE
        || !valid_bounded_identifier(&provider.route, 160)
        || !matches!(
            provider.readiness.as_str(),
            "pending" | "probing" | "ready" | "failed" | "released" | "expired"
        )
        || (state.status == "ready" && (!provider.claimable || provider.readiness != "ready"))
        || (state.status != "ready" && provider.claimable)
        || (state.status == "failed" && state.error.is_none())
        || (!terminal
            && state.status != "pending"
            && state.status != "probing"
            && state.status != "ready")
    {
        return Err(contract_error(()));
    }
    Ok(())
}

fn valid_bounded_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn validate_create_location(
    location: Option<&str>,
    control_base: &Url,
    expected: &Url,
) -> Result<(), GnosisError> {
    let Some(location) = location else {
        return Ok(());
    };
    if location.is_empty() || location.len() > 512 {
        return Err(contract_error(()));
    }
    let resolved = control_base.join(location).map_err(contract_error)?;
    if &resolved == expected {
        Ok(())
    } else {
        Err(contract_error(()))
    }
}

async fn claim_and_probe(
    client: &reqwest::Client,
    control_base: &Url,
    control_credential: &HeaderValue,
    identity: &ConnectionIdentity,
    audience: &str,
    control_is_loopback: bool,
    cancellation: &RunCancellation,
) -> Result<ProviderDescriptor, GnosisError> {
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

async fn probe_provider_health(
    client: &reqwest::Client,
    descriptor: &ProviderDescriptor,
    cancellation: &RunCancellation,
) -> Result<(), GnosisError> {
    let credential = provider_credential(&descriptor.credential.token)?;
    let health = send_json_response::<ProviderHealth>(
        client,
        Method::GET,
        Url::parse(&descriptor.health.url).map_err(contract_error)?,
        &credential,
        None,
        None,
        cancellation,
    )
    .await?;
    if health.status != StatusCode::OK || !health.value.ready || !health.value.accepting_requests {
        return Err(GnosisError::new(
            ErrorKind::Unavailable,
            "The gnosis provider did not pass semantic readiness checks.",
        ));
    }
    Ok(())
}

async fn cancellable_sleep(
    duration: Duration,
    cancellation: &RunCancellation,
) -> Result<(), GnosisError> {
    tokio::select! {
        _ = cancellation.cancelled() => Err(GnosisError::new(ErrorKind::Cancelled, "The gnosis provider connection was cancelled.")),
        _ = tokio::time::sleep(duration) => Ok(()),
    }
}

fn validate_profiles(profiles: &AgentProfiles) -> Result<(), GnosisError> {
    if profiles.contract_version != "agent-connection.v1" {
        return Err(contract_error(()));
    }
    let mut matching = profiles
        .profiles
        .iter()
        .filter(|profile| profile.id == AGENT_PROFILE);
    let profile = matching.next().ok_or_else(|| {
        GnosisError::new(
            ErrorKind::Contract,
            "gnosis does not advertise the required deep-reasoning provider profile.",
        )
    })?;
    if matching.next().is_some()
        || profile.providers.len() != 1
        || profile.providers[0].name != "llm"
        || profile.providers[0].capability != PROFILE_CAPABILITY
        || profile.providers[0].protocol != "openai.chat-completions.v1"
        || profile.providers[0].model != AGENT_PROFILE
    {
        return Err(contract_error(()));
    }
    Ok(())
}

fn select_audience(audiences: &[String]) -> Result<&str, GnosisError> {
    let mut matching = audiences
        .iter()
        .filter(|audience| audience.as_str() == AUDIENCE);
    match (matching.next(), matching.next()) {
        (Some(audience), None) => Ok(audience.as_str()),
        _ => Err(GnosisError::new(
            ErrorKind::Contract,
            "gnosis does not advertise exactly one saaa-desktop audience.",
        )),
    }
}

fn validate_claim(
    claim: ConnectionClaim,
    expected: &ConnectionIdentity,
    expected_audience: &str,
    control_is_loopback: bool,
) -> Result<ProviderDescriptor, GnosisError> {
    let claim_expires_at =
        chrono::DateTime::parse_from_rfc3339(&claim.expires_at).map_err(contract_error)?;
    if claim.id != expected.id
        || claim.allocation_id != expected.allocation_id
        || claim.status != "ready"
        || claim.audience != expected_audience
        || claim_expires_at <= chrono::Utc::now()
        || claim_expires_at != expected.expires_at
    {
        return Err(contract_error(()));
    }
    let mut matching = claim.providers.into_iter().filter(|provider| {
        provider.name == "llm"
            && provider.capability == PROFILE_CAPABILITY
            && provider.protocol == "openai.chat-completions.v1"
    });
    let descriptor = matching.next().ok_or_else(|| contract_error(()))?;
    if matching.next().is_some() {
        return Err(contract_error(()));
    }
    let base_url = Url::parse(&descriptor.base_url).map_err(contract_error)?;
    let expected_scheme = base_url.scheme();
    let expected_host = base_url.host_str().ok_or_else(|| contract_error(()))?;
    let expected_port = base_url
        .port_or_known_default()
        .ok_or_else(|| contract_error(()))?;
    let expires_at = chrono::DateTime::parse_from_rfc3339(&descriptor.credential.expires_at)
        .map_err(contract_error)?;
    let expected_health_url = provider_health_url(&base_url, &expected.id, &descriptor.name)?;
    if descriptor.api_style != "openai"
        || expected_scheme != "http"
        || descriptor.scheme != expected_scheme
        || descriptor.host != expected_host
        || descriptor.port != expected_port
        || (!control_is_loopback && descriptor.port != CONTROL_PORT)
        || base_url.path() != "/v1"
        || base_url.query().is_some()
        || base_url.fragment().is_some()
        || !base_url.username().is_empty()
        || base_url.password().is_some()
        || !url_is_local(&base_url)
        || (!control_is_loopback && url_is_loopback(&base_url))
        || descriptor.model != AGENT_PROFILE
        || descriptor.health.url != expected_health_url.as_str()
        || descriptor.health.kind != "semantic-inference"
        || descriptor.health.max_age_ms == 0
        || descriptor.health.max_age_ms > 60_000
        || descriptor.credential.r#type != "bearer"
        || descriptor.credential.token.is_empty()
        || descriptor.credential.token.len() > 4_096
        || descriptor
            .credential
            .token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace())
        || expires_at <= chrono::Utc::now()
        || expires_at != claim_expires_at
        || expires_at != expected.expires_at
        || descriptor.configuration.kind != "openai-provider-v1"
        || descriptor.configuration.fields.base_url != descriptor.base_url
        || descriptor.configuration.fields.model != descriptor.model
        || descriptor.configuration.secret_fields.api_key != "credential.token"
    {
        let message = if !control_is_loopback && url_is_loopback(&base_url) {
            "gnosis returned a same-host provider address; configure a LAN audience for SAAA."
        } else {
            "gnosis returned an invalid provider connection descriptor."
        };
        return Err(GnosisError::new(ErrorKind::Contract, message));
    }
    Ok(descriptor)
}

async fn send_json_response<T: for<'de> Deserialize<'de>>(
    client: &reqwest::Client,
    method: Method,
    url: Url,
    credential: &HeaderValue,
    extra_header: Option<(&str, &str)>,
    body: Option<&Value>,
    cancellation: &RunCancellation,
) -> Result<JsonResponse<T>, GnosisError> {
    let mut request = client
        .request(method, url)
        .header(AUTHORIZATION, credential.clone())
        .timeout(REQUEST_TIMEOUT);
    if let Some((name, value)) = extra_header {
        request = request.header(name, value);
    }
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(GnosisError::new(ErrorKind::Cancelled, "The gnosis provider connection was cancelled.")),
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
        Ok::<_, GnosisError>((
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

fn is_json_content_type(value: &str) -> bool {
    value
        .split(';')
        .next()
        .is_some_and(|media_type| media_type.trim() == "application/json")
}

fn parse_retry_after(value: Option<&HeaderValue>) -> Result<Option<Duration>, GnosisError> {
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

fn bounded_header(
    value: Option<&HeaderValue>,
    max_len: usize,
) -> Result<Option<String>, GnosisError> {
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

async fn release_connection(
    client: &reqwest::Client,
    url: &Url,
    credential: &HeaderValue,
) -> Result<(), GnosisError> {
    let response = client
        .delete(url.clone())
        .header(AUTHORIZATION, credential.clone())
        .timeout(RELEASE_TIMEOUT)
        .send()
        .await
        .map_err(classify_transport)?;
    if response.status() == StatusCode::NO_CONTENT {
        Ok(())
    } else {
        Err(classify_status(response.status(), ""))
    }
}

async fn error_after_release(
    mut error: GnosisError,
    client: &reqwest::Client,
    url: &Url,
    credential: &HeaderValue,
) -> GnosisError {
    if let Err(release_error) = release_connection(client, url, credential).await {
        error.release_failure = Some(release_error.kind);
    }
    error
}

async fn read_limited(
    response: reqwest::Response,
    cancellation: &RunCancellation,
) -> Result<Vec<u8>, GnosisError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err(GnosisError::new(ErrorKind::Cancelled, "The gnosis provider connection was cancelled.")),
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

fn control_credential() -> Result<HeaderValue, GnosisError> {
    let token = Zeroizing::new(env::var(API_TOKEN_ENV).map_err(|_| {
        GnosisError::new(
            ErrorKind::Authentication,
            "LARM_API_TOKEN is required to resolve gnosis provider settings.",
        )
    })?);
    if token.is_empty()
        || token.len() > 4_096
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(GnosisError::new(
            ErrorKind::Authentication,
            "LARM_API_TOKEN is invalid.",
        ));
    }
    provider_credential(token.as_str())
        .map_err(|_| GnosisError::new(ErrorKind::Authentication, "LARM_API_TOKEN is invalid."))
}

fn provider_credential(token: &str) -> Result<HeaderValue, GnosisError> {
    if token.is_empty()
        || token.len() > 4_096
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(GnosisError::new(
            ErrorKind::Authentication,
            "The gnosis provider credential is invalid.",
        ));
    }
    let bearer = Zeroizing::new(format!("Bearer {token}"));
    let mut value = HeaderValue::from_str(bearer.as_str()).map_err(|_| {
        GnosisError::new(
            ErrorKind::Authentication,
            "The gnosis provider credential is invalid.",
        )
    })?;
    value.set_sensitive(true);
    Ok(value)
}

pub(crate) fn control_base_url(host: &str) -> Result<Url, GnosisError> {
    let host = host.trim();
    if host.is_empty()
        || host.len() > 253
        || host.starts_with('-')
        || host.contains(['/', '@', '?', '#'])
        || host.chars().any(char::is_whitespace)
        || host.contains(':')
    {
        return Err(GnosisError::new(
            ErrorKind::Contract,
            "gnosis host must be a hostname or private IP address without a scheme, port, or path.",
        ));
    }
    let url = Url::parse(&format!("http://{host}:{CONTROL_PORT}/")).map_err(contract_error)?;
    if !url_is_local(&url) {
        return Err(GnosisError::new(
            ErrorKind::Contract,
            "gnosis host must be a loopback, private-network, .local, or single-label host.",
        ));
    }
    Ok(url)
}

fn url_is_local(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => valid_local_hostname(host),
        Some(Host::Ipv4(address)) => {
            address.is_loopback() || address.is_private() || address.is_link_local()
        }
        Some(Host::Ipv6(address)) => {
            address.is_loopback()
                || (address.segments()[0] & 0xfe00) == 0xfc00
                || (address.segments()[0] & 0xffc0) == 0xfe80
        }
        None => false,
    }
}

fn valid_local_hostname(host: &str) -> bool {
    let labels = host.split('.').collect::<Vec<_>>();
    (labels.len() == 1 || host.ends_with(".local"))
        && labels.iter().all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn url_is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host == "localhost",
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn validate_connection_id(id: &str) -> Result<(), GnosisError> {
    if id.is_empty()
        || id.len() > 192
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(contract_error(()));
    }
    Ok(())
}

fn connection_resource_url(control_base: &Url, id: &str) -> Result<Url, GnosisError> {
    validate_connection_id(id)?;
    let mut url = control_base.clone();
    url.path_segments_mut()
        .map_err(contract_error)?
        .extend(["v1", "agent-connections", id]);
    Ok(url)
}

fn connection_claim_url(control_base: &Url, id: &str) -> Result<Url, GnosisError> {
    let mut url = connection_resource_url(control_base, id)?;
    url.path_segments_mut()
        .map_err(contract_error)?
        .push("claim");
    Ok(url)
}

fn connection_renew_url(control_base: &Url, id: &str) -> Result<Url, GnosisError> {
    let mut url = connection_resource_url(control_base, id)?;
    url.path_segments_mut()
        .map_err(contract_error)?
        .push("renew");
    Ok(url)
}

fn provider_health_url(
    provider_base: &Url,
    connection_id: &str,
    provider_name: &str,
) -> Result<Url, GnosisError> {
    validate_connection_id(connection_id)?;
    if provider_name != "llm" {
        return Err(contract_error(()));
    }
    let mut url = provider_base.clone();
    url.path_segments_mut().map_err(contract_error)?.extend([
        "agent-connections",
        connection_id,
        "providers",
        provider_name,
        "health",
    ]);
    Ok(url)
}

fn classify_transport(error: reqwest::Error) -> GnosisError {
    if error.is_timeout() {
        GnosisError::new(
            ErrorKind::Timeout,
            "The gnosis configuration API request timed out.",
        )
    } else {
        GnosisError::new(
            ErrorKind::Network,
            "Could not reach the gnosis configuration API.",
        )
    }
}

fn classify_status(status: StatusCode, code: &str) -> GnosisError {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return GnosisError::new(ErrorKind::Authentication, "gnosis rejected LARM_API_TOKEN.");
    }
    if matches!(status, StatusCode::NOT_FOUND | StatusCode::GONE) {
        return GnosisError::new(
            ErrorKind::StaleConnection,
            "The gnosis provider connection is no longer active.",
        );
    }
    if status == StatusCode::CONFLICT {
        return classify_api_error(code);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return GnosisError::new(
            ErrorKind::Capacity,
            "gnosis cannot allocate the requested provider right now.",
        );
    }
    if status == StatusCode::SERVICE_UNAVAILABLE {
        return GnosisError::new(
            ErrorKind::Unavailable,
            "The gnosis provider service is not ready.",
        );
    }
    if status.is_server_error() {
        return GnosisError::new(
            ErrorKind::Upstream,
            "gnosis could not resolve the provider connection.",
        );
    }
    classify_api_error(code)
}

fn classify_api_error(code: &str) -> GnosisError {
    match code {
        "capacity_exhausted" | "admission_denied" | "provider_busy" => GnosisError::new(
            ErrorKind::Capacity,
            "gnosis cannot allocate the requested provider right now.",
        ),
        "connection_auth_not_configured" | "unauthorized" | "forbidden" => {
            GnosisError::new(ErrorKind::Authentication, "gnosis rejected LARM_API_TOKEN.")
        }
        "provider_semantic_not_ready" | "connection_not_ready" => GnosisError::new(
            ErrorKind::Unavailable,
            "The gnosis provider did not pass semantic readiness checks.",
        ),
        "connection_inactive"
        | "connection_expired"
        | "connection_released"
        | "connection_boot_epoch_mismatch"
        | "connection_not_found" => GnosisError::new(
            ErrorKind::StaleConnection,
            "The gnosis provider connection is no longer active.",
        ),
        _ => contract_error(()),
    }
}

fn contract_error<T>(_: T) -> GnosisError {
    GnosisError::new(
        ErrorKind::Contract,
        "gnosis returned an incompatible agent-connection response.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
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
            control_base_url("192.168.0.65")
                .expect("private host")
                .as_str(),
            "http://192.168.0.65:9810/"
        );
        assert_eq!(
            control_base_url("gnosis")
                .expect("single-label host")
                .as_str(),
            "http://gnosis:9810/"
        );
    }

    #[test]
    fn rejects_urls_ports_and_public_hosts() {
        for host in [
            "http://gnosis",
            "gnosis:8083",
            "example.com",
            "gnosis/path",
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
    fn accepts_a_private_http_endpoint_and_rejects_loopback_for_remote_gnosis() {
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

        let descriptor = validate_claim(claim("192.168.0.65"), &identity, AUDIENCE, false)
            .expect("private HTTP descriptor is accepted");
        assert_eq!(descriptor.base_url, "http://192.168.0.65:9810/v1");
        assert!(validate_claim(claim("127.0.0.1"), &identity, AUDIENCE, false).is_err());
        let wrong_identity = test_identity("different-id", &created_at, &expires_at);
        assert!(validate_claim(claim("192.168.0.65"), &wrong_identity, AUDIENCE, false).is_err());
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

        let error = match GnosisConnection::resolve_at(
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

        let connection = GnosisConnection::resolve_at(
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

        let error = match GnosisConnection::resolve_at(
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

        let mut connection = GnosisConnection::resolve_at(
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
    #[ignore = "operator-only live gnosis Agent Connection API canary"]
    async fn live_agent_connection_claim_and_chat() {
        let _environment = super::super::larm::test_environment_lock().lock().await;
        let host = env::var("SAAA_GNOSIS_HOST").expect("SAAA_GNOSIS_HOST is required");
        let connection = GnosisConnection::resolve(&host, Arc::new(RunCancellation::default()))
            .await
            .expect("live gnosis connection resolves");
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
