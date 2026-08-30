use crate::{ModelProviderSettings, ModelProvidersSettings};

pub(crate) fn validate_model_providers(settings: &ModelProvidersSettings) -> Result<(), String> {
    if !crate::providers::valid_conversation_reasoning_effort(&settings.reasoning_effort) {
        return Err("Reasoning effort must be low, medium, or xhigh".to_string());
    }
    validate_harness_address(&settings.harness.address)?;
    if settings.providers.is_empty() || settings.providers.len() > 20 {
        return Err("Between 1 and 20 model providers are required".to_string());
    }
    let mut ids = std::collections::HashSet::new();
    let mut enabled_larm_count = 0;
    let mut enabled_dynamic_lan_count = 0;
    for provider in &settings.providers {
        let provider_id = provider.id();
        if provider_id.is_empty()
            || provider_id.len() > 80
            || !provider_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
            || !ids.insert(provider_id)
        {
            return Err("Invalid or duplicate provider id".to_string());
        }
        if provider.label().trim().is_empty()
            || provider.label().trim() != provider.label()
            || provider.label().chars().count() > 120
            || provider.label().chars().any(char::is_control)
        {
            return Err(format!("Invalid provider label: {provider_id}"));
        }
        if !matches!(provider.location(), "local" | "cloud") {
            return Err(format!("Invalid provider location: {provider_id}"));
        }
        match provider {
            ModelProviderSettings::OpenAiCompatible(provider) => {
                if !matches!(provider.authentication.as_str(), "none" | "api-key") {
                    return Err(format!("Invalid authentication: {provider_id}"));
                }
                if provider.endpoint.len() > 2_048
                    || provider.model.chars().count() > 160
                    || provider.model.trim() != provider.model
                    || provider.model.chars().any(char::is_control)
                {
                    return Err(format!(
                        "Provider endpoint or model is too long: {provider_id}"
                    ));
                }
                if provider.enabled
                    && (provider.endpoint.trim().is_empty() || provider.model.trim().is_empty())
                {
                    return Err(format!(
                        "Enabled provider requires endpoint and model: {provider_id}"
                    ));
                }
                if provider.endpoint.trim().is_empty() {
                    continue;
                }
                let endpoint = url::Url::parse(&provider.endpoint)
                    .map_err(|_| format!("Invalid provider endpoint: {provider_id}"))?;
                if !endpoint.username().is_empty()
                    || endpoint.password().is_some()
                    || endpoint.query().is_some()
                    || endpoint.fragment().is_some()
                {
                    return Err(format!(
                        "Provider endpoint must not contain credentials, query, or fragment: {provider_id}"
                    ));
                }
                if !matches!(endpoint.scheme(), "http" | "https") {
                    return Err(format!(
                        "Provider endpoint must use HTTP or HTTPS: {provider_id}"
                    ));
                }
                if provider.location == "local" {
                    if endpoint.scheme() != "http"
                        || !match endpoint.host() {
                            Some(url::Host::Domain(host)) => {
                                host == "localhost"
                                    || host.ends_with(".local")
                                    || !host.contains('.')
                            }
                            Some(url::Host::Ipv4(address)) => {
                                address.is_loopback() || address.is_private()
                            }
                            Some(url::Host::Ipv6(address)) => {
                                address.is_loopback() || address.is_unique_local()
                            }
                            None => false,
                        }
                    {
                        return Err(format!(
                            "Local provider must use an HTTP loopback or private-network endpoint: {provider_id}"
                        ));
                    }
                } else if endpoint.scheme() != "https" {
                    return Err(format!("Cloud provider must use HTTPS: {provider_id}"));
                }
            }
            ModelProviderSettings::CloudAsr(provider) => {
                validate_cloud_provider(
                    provider_id,
                    &provider.location,
                    &provider.endpoint,
                    &provider.model,
                    &provider.authentication,
                )?;
                if provider.language != "auto" {
                    return Err(format!(
                        "ASR providers must use automatic language detection: {provider_id}"
                    ));
                }
            }
            ModelProviderSettings::CloudTts(provider) => {
                validate_cloud_provider(
                    provider_id,
                    &provider.location,
                    &provider.endpoint,
                    &provider.model,
                    &provider.authentication,
                )?;
                if provider.voice.trim().is_empty()
                    || provider.voice.trim() != provider.voice
                    || provider.voice.chars().count() > 160
                    || provider.voice.chars().any(char::is_control)
                {
                    return Err(format!("Invalid TTS voice: {provider_id}"));
                }
            }
            ModelProviderSettings::SystemTts(provider) => {
                if provider.location != "local"
                    || provider.voice.trim().is_empty()
                    || provider.voice.trim() != provider.voice
                    || provider.voice.chars().count() > 160
                    || provider.voice.chars().any(char::is_control)
                {
                    return Err(format!("Invalid system TTS provider: {provider_id}"));
                }
            }
            ModelProviderSettings::Larm(provider) => {
                if provider.enabled {
                    enabled_larm_count += 1;
                }
                if provider.location != "local"
                    || provider.base_url.len() > 2_048
                    || provider.token_env != "LARM_API_TOKEN"
                    || !(60..=3_600).contains(&provider.allocation_ttl_seconds)
                    || !(1..=300).contains(&provider.allocation_startup_timeout_seconds)
                    || provider.allow_fallback_by_default
                    || provider.deployment_policy != "existing-only"
                {
                    return Err(format!(
                        "LARM provider violates the fixed security policy: {provider_id}"
                    ));
                }
                let base_url = url::Url::parse(&provider.base_url)
                    .map_err(|_| format!("Invalid LARM base URL: {provider_id}"))?;
                let numeric_loopback = matches!(
                    base_url.host(),
                    Some(url::Host::Ipv4(address))
                        if address == std::net::Ipv4Addr::LOCALHOST
                ) || matches!(
                    base_url.host(),
                    Some(url::Host::Ipv6(address))
                        if address == std::net::Ipv6Addr::LOCALHOST
                );
                if base_url.scheme() != "http"
                    || !numeric_loopback
                    || base_url.port().is_none()
                    || !base_url.username().is_empty()
                    || base_url.password().is_some()
                    || base_url.query().is_some()
                    || base_url.fragment().is_some()
                    || base_url.path() != "/"
                {
                    return Err(format!(
                        "LARM base URL must be an explicit numeric HTTP loopback origin: {provider_id}"
                    ));
                }
            }
            ModelProviderSettings::DynamicLan(provider) => {
                if provider.enabled {
                    enabled_dynamic_lan_count += 1;
                }
                if provider.location != "local"
                    || crate::providers::dynamic_lan::control_base_url(&provider.host).is_err()
                {
                    return Err(format!(
                        "dynamic LAN provider requires only a private-network host: {provider_id}"
                    ));
                }
            }
        }
    }
    if enabled_larm_count > 1 {
        return Err("Only one LARM provider may be enabled".to_string());
    }
    if enabled_dynamic_lan_count > 1 {
        return Err("Only one dynamic LAN provider may be enabled".to_string());
    }
    Ok(())
}

fn validate_harness_address(address: &str) -> Result<(), String> {
    if address.is_empty() {
        return Ok(());
    }
    if address.len() > 2_048 {
        return Err("Harness address is too long".to_string());
    }
    let url = url::Url::parse(address).map_err(|_| "Invalid harness address".to_string())?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.scheme(), "http" | "https")
    {
        return Err(
            "Harness address must be an HTTP(S) URL without credentials, query, or fragment"
                .to_string(),
        );
    }
    if url.scheme() == "http" {
        let is_private = match url.host() {
            Some(url::Host::Domain(host)) => {
                host == "localhost" || host.ends_with(".local") || !host.contains('.')
            }
            Some(url::Host::Ipv4(address)) => address.is_loopback() || address.is_private(),
            Some(url::Host::Ipv6(address)) => address.is_loopback() || address.is_unique_local(),
            None => false,
        };
        if !is_private {
            return Err("Public harness addresses must use HTTPS".to_string());
        }
    }
    Ok(())
}

fn validate_cloud_provider(
    provider_id: &str,
    location: &str,
    endpoint: &str,
    model: &str,
    authentication: &str,
) -> Result<(), String> {
    if location != "cloud"
        || endpoint.len() > 2_048
        || model.trim().is_empty()
        || model.trim() != model
        || model.chars().count() > 160
        || model.chars().any(char::is_control)
        || !matches!(authentication, "none" | "api-key")
    {
        return Err(format!("Invalid cloud provider metadata: {provider_id}"));
    }
    let endpoint = url::Url::parse(endpoint)
        .map_err(|_| format!("Invalid cloud provider endpoint: {provider_id}"))?;
    if endpoint.scheme() != "https"
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(format!(
            "Cloud provider must use a credential-free HTTPS endpoint: {provider_id}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(provider: ModelProviderSettings) -> ModelProvidersSettings {
        ModelProvidersSettings {
            harness: crate::HarnessSettings {
                address: "http://localhost:9810".to_string(),
            },
            providers: vec![provider],
            reasoning_effort: crate::providers::default_conversation_reasoning_effort(),
        }
    }

    #[test]
    fn local_names_match_the_frontend_and_metadata_is_not_silently_trimmed() {
        let mut provider = crate::test_support::provider("local", "local");
        let ModelProviderSettings::OpenAiCompatible(value) = &mut provider else {
            unreachable!("fixture must remain OpenAI-compatible");
        };
        value.endpoint = "http://llm.local:8080/v1".to_string();
        assert!(validate_model_providers(&settings(provider.clone())).is_ok());
        let ModelProviderSettings::OpenAiCompatible(value) = &mut provider else {
            unreachable!("fixture must remain OpenAI-compatible");
        };
        value.model = " model".to_string();
        assert!(validate_model_providers(&settings(provider)).is_err());
    }
}
