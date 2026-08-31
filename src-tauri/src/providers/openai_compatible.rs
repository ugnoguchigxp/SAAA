use crate::OpenAiCompatibleProviderSettings;
use zeroize::Zeroizing;

mod probe;
pub(crate) use probe::probe_model_provider;

#[cfg(test)]
/// Projects one SSE event according to the event-stream specification. Multiple
/// `data:` fields belong to the same event and are joined with a newline; treating
/// them as independent JSON documents silently accepts truncated provider output.
pub(crate) fn sse_event_data(event: &str) -> Option<String> {
    let fields = event.lines().filter_map(|line| {
        let value = line.strip_prefix("data:")?;
        Some(value.strip_prefix(' ').unwrap_or(value))
    });
    let data = fields.collect::<Vec<_>>();
    (!data.is_empty()).then(|| data.join("\n"))
}

pub(crate) fn provider_models_url(endpoint: &str) -> Result<String, String> {
    provider_operation_url(endpoint, "models")
}

fn provider_operation_url(endpoint: &str, operation: &str) -> Result<String, String> {
    let mut url =
        url::Url::parse(endpoint).map_err(|_| "Provider endpoint is invalid".to_string())?;
    let mut path = url.path().trim_end_matches('/').to_string();
    if path.ends_with("/chat/completions") {
        path.truncate(path.len() - "/chat/completions".len());
    } else if path.ends_with("/models") {
        path.truncate(path.len() - "/models".len());
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
    fn sse_data_fields_are_joined_as_one_event() {
        let event = "event: message\ndata: {\"choices\":[\ndata: {\"index\":0,\"delta\":{}}]}";
        assert_eq!(
            sse_event_data(event).as_deref(),
            Some("{\"choices\":[\n{\"index\":0,\"delta\":{}}]}")
        );
        assert_eq!(sse_event_data(": heartbeat"), None);
    }
}
