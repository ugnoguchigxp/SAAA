use super::*;

pub(crate) fn legacy_dynamic_lan_host(address: &str) -> Result<Option<String>, String> {
    let base = validate_address(address)?;
    let is_legacy_address = base.scheme() == "http"
        && base.port() == Some(crate::providers::dynamic_lan::CONTROL_PORT)
        && base.path() == "/"
        && !matches!(base.host(), Some(url::Host::Ipv6(_)));
    Ok(is_legacy_address.then(|| base.host_str().unwrap_or_default().to_string()))
}

pub(super) fn validate_address(address: &str) -> Result<url::Url, String> {
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

pub(super) fn validate_descriptor(
    base: &url::Url,
    descriptor: &HarnessDescriptor,
) -> Result<(), String> {
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
