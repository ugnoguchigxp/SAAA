use super::{ModelStreamContext, ProviderAttemptError, ProviderFailureKind as Failure};
use serde_json::Value;
use std::time::Duration;

pub(super) async fn run(
    request: reqwest::RequestBuilder,
    timeout_ms: u64,
    context: ModelStreamContext<'_>,
    attempt: &mut super::observation::Attempt<'_>,
) -> Result<String, ProviderAttemptError> {
    let cancellation = context.cancellation.clone();
    let response = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(ProviderAttemptError::Cancelled { output_started: false }),
        result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
            let mut response = request.send().await.map_err(|e| if e.is_connect() { Failure::Connect } else { Failure::Network })?;
            if !response.status().is_success() {
                return Err(crate::providers::http::status_failure(response.status().as_u16()));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| Failure::ResponseInterrupted)? {
                if bytes.len().saturating_add(chunk.len()) > 2 * 1024 * 1024 {
                    return Err(Failure::RequestTooLarge);
                }
                bytes.extend_from_slice(&chunk);
            }
            serde_json::from_slice::<Value>(&bytes).map_err(|_| Failure::Protocol)
        }) => result.map_err(|_| ProviderAttemptError::failed(Failure::Timeout, false))?
            .map_err(|kind| ProviderAttemptError::failed_with_detail(
                kind, false, (kind == Failure::Protocol).then_some("invalid-chat-json"),
            ))?,
    };
    super::response_json::content(&response, attempt)
}
