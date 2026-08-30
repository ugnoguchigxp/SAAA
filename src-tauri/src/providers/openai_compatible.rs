use std::env;

use crate::OpenAiCompatibleProviderSettings;

mod probe;
pub(crate) use probe::probe_model_provider;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum SseDrainError {
    InvalidUtf8,
    EventTooLarge,
}

pub(crate) fn drain_sse_events(
    buffer: &mut Vec<u8>,
    event_limit: usize,
) -> Result<Vec<String>, SseDrainError> {
    let mut events = Vec::new();
    loop {
        let lf = buffer.windows(2).position(|window| window == b"\n\n");
        let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
        let boundary = match (lf, crlf) {
            (Some(lf), Some(crlf)) if lf < crlf => Some((lf, 2)),
            (Some(_), Some(crlf)) => Some((crlf, 4)),
            (Some(lf), None) => Some((lf, 2)),
            (None, Some(crlf)) => Some((crlf, 4)),
            (None, None) => None,
        };
        let Some((index, delimiter_length)) = boundary else {
            break;
        };
        if index > event_limit {
            return Err(SseDrainError::EventTooLarge);
        }
        let drained = buffer.drain(..index + delimiter_length).collect::<Vec<_>>();
        events.push(
            String::from_utf8(drained[..index].to_vec()).map_err(|_| SseDrainError::InvalidUtf8)?,
        );
    }
    Ok(events)
}

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

pub(crate) fn provider_chat_url(endpoint: &str) -> Result<String, String> {
    provider_operation_url(endpoint, "chat/completions")
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

pub(crate) fn provider_api_key(provider: &OpenAiCompatibleProviderSettings) -> Option<String> {
    let suffix = provider_environment_suffix(&provider.id);
    env::var(format!("SAAA_PROVIDER_{suffix}_API_KEY"))
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            (provider.location == "cloud")
                .then(|| env::var("OPENAI_API_KEY").ok())
                .flatten()
                .filter(|value| !value.is_empty())
        })
}

pub(crate) fn provider_environment_suffix(provider_id: &str) -> String {
    provider_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_handles_lf_crlf_and_partial_events() {
        let mut buffer = b"data: {\"a\":1}\r\n\r\ndata: {\"b\":2}\n\ndata: partial".to_vec();
        let events = drain_sse_events(&mut buffer, 1_048_576).expect("SSE events parse");
        assert_eq!(events, ["data: {\"a\":1}", "data: {\"b\":2}"]);
        assert_eq!(buffer, b"data: partial");
    }

    #[test]
    fn sse_parser_applies_the_limit_to_each_event() {
        let event = format!("data: {}\n\n", "x".repeat(600 * 1_024));
        let mut buffer = [event.as_bytes(), event.as_bytes()].concat();
        let events = drain_sse_events(&mut buffer, 1_048_576)
            .expect("two individually bounded events parse");
        assert_eq!(events.len(), 2);
        assert!(buffer.is_empty());

        let mut oversized = format!("data: {}\n\n", "x".repeat(1_048_577)).into_bytes();
        assert_eq!(
            drain_sse_events(&mut oversized, 1_048_576),
            Err(SseDrainError::EventTooLarge)
        );
        let mut invalid_utf8 = b"data: \xff\n\n".to_vec();
        assert_eq!(
            drain_sse_events(&mut invalid_utf8, 1_048_576),
            Err(SseDrainError::InvalidUtf8)
        );
    }

    #[test]
    fn sse_parser_preserves_utf8_split_across_network_chunks() {
        let event = "data: {\"choices\":[{\"delta\":{\"content\":\"こんにちは\"}}]}\n\n";
        let bytes = event.as_bytes();
        let split = bytes
            .windows("こ".len())
            .position(|window| window == "こ".as_bytes())
            .expect("multibyte character exists")
            + 1;
        let mut buffer = bytes[..split].to_vec();
        assert!(drain_sse_events(&mut buffer, 1_048_576)
            .expect("partial UTF-8 remains buffered")
            .is_empty());
        buffer.extend_from_slice(&bytes[split..]);
        assert_eq!(
            drain_sse_events(&mut buffer, 1_048_576).expect("complete UTF-8 event parses"),
            [event.trim_end()]
        );
    }

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
