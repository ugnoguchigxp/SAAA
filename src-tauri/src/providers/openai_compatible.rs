use crate::OpenAiCompatibleProviderSettings;
use zeroize::Zeroizing;

mod probe;
pub(crate) use probe::{probe_model_provider, probe_model_provider_with_api_key};

pub(crate) fn provider_operation_url(endpoint: &str, operation: &str) -> Result<String, String> {
    saaa_larm_session::http_api::operation_url(endpoint, operation)
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
