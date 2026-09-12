use crate::OpenAiCompatibleProviderSettings;
use zeroize::Zeroizing;

mod probe;
pub(crate) use probe::{probe_model_provider, probe_model_provider_with_api_key};

pub(crate) fn provider_operation_url(endpoint: &str, operation: &str) -> Result<String, String> {
    let mut url =
        url::Url::parse(endpoint).map_err(|_| "Provider endpoint is invalid".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Provider base URL must be HTTP(S) without credentials, query or fragment".to_string(),
        );
    }
    let mut path = url.path().trim_end_matches('/').to_string();
    for suffix in [
        "/chat/completions",
        "/audio/transcriptions",
        "/audio/speech",
        "/models",
    ] {
        if path.ends_with(suffix) {
            path.truncate(path.len() - suffix.len());
            break;
        }
    }
    if !path.ends_with("/v1") {
        path.push_str("/v1");
    }
    path.push('/');
    path.push_str(operation);
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

pub(crate) fn provider_api_key(
    provider: &OpenAiCompatibleProviderSettings,
) -> Result<Option<Zeroizing<String>>, String> {
    if provider.authentication == "none" {
        return Ok(None);
    }
    crate::credentials::load_api_key(&provider.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_operation_urls_preserve_proxy_prefix_and_origin() {
        for endpoint in [
            "http://localhost:8000/proxy",
            "http://localhost:8000/proxy/v1/",
            "http://localhost:8000/proxy/v1/chat/completions",
            "http://localhost:8000/proxy/v1/models",
            "http://localhost:8000/proxy/v1/audio/transcriptions",
            "http://localhost:8000/proxy/v1/audio/speech",
        ] {
            for operation in ["chat/completions", "audio/transcriptions", "audio/speech"] {
                assert_eq!(
                    provider_operation_url(endpoint, operation).unwrap(),
                    format!("http://localhost:8000/proxy/v1/{operation}")
                );
            }
        }
        for endpoint in [
            "ws://localhost/v1",
            "http://token@localhost/v1",
            "http://localhost/v1?token=secret",
            "http://localhost/v1#fragment",
        ] {
            assert!(provider_operation_url(endpoint, "chat/completions").is_err());
        }
    }
}
