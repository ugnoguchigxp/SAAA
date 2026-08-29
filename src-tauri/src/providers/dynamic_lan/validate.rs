use url::{Host, Url};

use super::{
    contract_error, AgentProfiles, ConnectionClaim, ConnectionIdentity, ConnectionState,
    DynamicLanError, ErrorKind, ProviderDescriptor, AGENT_PROFILE, AUDIENCE,
    CLOCK_SKEW_TOLERANCE_SECONDS, CONNECTION_TTL_SECONDS, CONTROL_PORT, PROFILE_CAPABILITY,
};

pub(crate) fn validate_config_revision(revision: Option<&str>) -> Result<(), DynamicLanError> {
    let revision = revision.ok_or_else(|| contract_error(()))?;
    validate_revision(revision)
}

pub(crate) fn validate_revision(revision: &str) -> Result<(), DynamicLanError> {
    if revision.len() == 64 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(contract_error(()))
    }
}

pub(crate) fn validate_initial_state(
    state: &ConnectionState,
    expected_audience: &str,
) -> Result<ConnectionIdentity, DynamicLanError> {
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

pub(crate) fn validate_successor_state(
    state: &ConnectionState,
    expected: &ConnectionIdentity,
    expected_audience: &str,
) -> Result<(), DynamicLanError> {
    validate_state_shape(state, expected_audience)?;
    let created_at =
        chrono::DateTime::parse_from_rfc3339(&state.created_at).map_err(contract_error)?;
    let expires_at =
        chrono::DateTime::parse_from_rfc3339(&state.expires_at).map_err(contract_error)?;
    if state.boot_epoch != expected.boot_epoch {
        return Err(DynamicLanError::new(
            ErrorKind::StaleConnection,
            "The dynamic_lan daemon restarted while resolving the provider connection.",
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

pub(crate) fn validate_renewed_state(
    state: &ConnectionState,
    expected: &ConnectionIdentity,
    expected_audience: &str,
) -> Result<ConnectionIdentity, DynamicLanError> {
    validate_state_shape(state, expected_audience)?;
    let created_at =
        chrono::DateTime::parse_from_rfc3339(&state.created_at).map_err(contract_error)?;
    let expires_at =
        chrono::DateTime::parse_from_rfc3339(&state.expires_at).map_err(contract_error)?;
    if state.boot_epoch != expected.boot_epoch {
        return Err(DynamicLanError::new(
            ErrorKind::StaleConnection,
            "The dynamic_lan daemon restarted while renewing the provider connection.",
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

pub(crate) fn validate_state_shape(
    state: &ConnectionState,
    expected_audience: &str,
) -> Result<(), DynamicLanError> {
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

pub(crate) fn valid_bounded_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

pub(crate) fn validate_create_location(
    location: Option<&str>,
    control_base: &Url,
    expected: &Url,
) -> Result<(), DynamicLanError> {
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

pub(crate) fn validate_profiles(profiles: &AgentProfiles) -> Result<(), DynamicLanError> {
    if profiles.contract_version != "agent-connection.v1" {
        return Err(contract_error(()));
    }
    let mut matching = profiles
        .profiles
        .iter()
        .filter(|profile| profile.id == AGENT_PROFILE);
    let profile = matching.next().ok_or_else(|| {
        DynamicLanError::new(
            ErrorKind::Contract,
            "dynamic_lan does not advertise the required deep-reasoning provider profile.",
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

pub(crate) fn select_audience(audiences: &[String]) -> Result<&str, DynamicLanError> {
    let mut matching = audiences
        .iter()
        .filter(|audience| audience.as_str() == AUDIENCE);
    match (matching.next(), matching.next()) {
        (Some(audience), None) => Ok(audience.as_str()),
        _ => Err(DynamicLanError::new(
            ErrorKind::Contract,
            "dynamic_lan does not advertise exactly one saaa-desktop audience.",
        )),
    }
}

pub(crate) fn validate_claim(
    claim: ConnectionClaim,
    expected: &ConnectionIdentity,
    expected_audience: &str,
    control_is_loopback: bool,
) -> Result<ProviderDescriptor, DynamicLanError> {
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
            "dynamic_lan returned a same-host provider address; configure a LAN audience for SAAA."
        } else {
            "dynamic_lan returned an invalid provider connection descriptor."
        };
        return Err(DynamicLanError::new(ErrorKind::Contract, message));
    }
    Ok(descriptor)
}

pub(crate) fn control_base_url(host: &str) -> Result<Url, DynamicLanError> {
    let host = host.trim();
    if host.is_empty()
        || host.len() > 253
        || host.starts_with('-')
        || host.contains(['/', '@', '?', '#'])
        || host.chars().any(char::is_whitespace)
        || host.contains(':')
    {
        return Err(DynamicLanError::new(
            ErrorKind::Contract,
            "dynamic_lan host must be a hostname or private IP address without a scheme, port, or path.",
        ));
    }
    let url = Url::parse(&format!("http://{host}:{CONTROL_PORT}/")).map_err(contract_error)?;
    if !url_is_local(&url) {
        return Err(DynamicLanError::new(
            ErrorKind::Contract,
            "dynamic_lan host must be a loopback, private-network, .local, or single-label host.",
        ));
    }
    Ok(url)
}

pub(crate) fn url_is_local(url: &Url) -> bool {
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

pub(crate) fn valid_local_hostname(host: &str) -> bool {
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

pub(crate) fn url_is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host == "localhost",
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

pub(crate) fn validate_connection_id(id: &str) -> Result<(), DynamicLanError> {
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

pub(crate) fn connection_resource_url(
    control_base: &Url,
    id: &str,
) -> Result<Url, DynamicLanError> {
    validate_connection_id(id)?;
    let mut url = control_base.clone();
    url.path_segments_mut()
        .map_err(contract_error)?
        .extend(["v1", "agent-connections", id]);
    Ok(url)
}

pub(crate) fn connection_claim_url(control_base: &Url, id: &str) -> Result<Url, DynamicLanError> {
    let mut url = connection_resource_url(control_base, id)?;
    url.path_segments_mut()
        .map_err(contract_error)?
        .push("claim");
    Ok(url)
}

pub(crate) fn connection_renew_url(control_base: &Url, id: &str) -> Result<Url, DynamicLanError> {
    let mut url = connection_resource_url(control_base, id)?;
    url.path_segments_mut()
        .map_err(contract_error)?
        .push("renew");
    Ok(url)
}

pub(crate) fn provider_health_url(
    provider_base: &Url,
    connection_id: &str,
    provider_name: &str,
) -> Result<Url, DynamicLanError> {
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
