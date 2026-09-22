use super::stream::ProviderFailureKind as Failure;
use crate::RunCancellation;
use std::time::Duration;

pub(crate) async fn send(
    request: reqwest::RequestBuilder,
    cancellation: &RunCancellation,
    allow_retry: bool,
) -> Result<reqwest::Response, Failure> {
    send_with(
        || request.try_clone().ok_or(Failure::Internal),
        cancellation,
        allow_retry,
    )
    .await
}

pub(crate) async fn send_with(
    request: impl Fn() -> Result<reqwest::RequestBuilder, Failure>,
    cancellation: &RunCancellation,
    allow_retry: bool,
) -> Result<reqwest::Response, Failure> {
    for attempt in 0..3 {
        let next = request()?;
        let response = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err(Failure::Cancelled),
            result = next.send() => result.map_err(|error| if error.is_timeout() { Failure::Timeout } else { Failure::Connect })?,
        };
        if response.status().is_success() {
            return Ok(response);
        }
        let kind = status_failure(response.status().as_u16());
        if allow_retry && attempt < 2 && matches!(response.status().as_u16(), 429 | 503) {
            let delay = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(retry_delay)
                .unwrap_or(Duration::from_millis(250 << attempt));
            // Never retry earlier than Retry-After; a long wait is surfaced to the caller.
            if delay > Duration::from_secs(5) {
                return Err(kind);
            }
            drop(response);
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(Failure::Cancelled),
                _ = tokio::time::sleep(delay) => {},
            }
        } else {
            return Err(kind);
        }
    }
    Err(Failure::Internal)
}

fn retry_delay(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some(
        date.signed_duration_since(chrono::Utc::now())
            .to_std()
            .unwrap_or_default(),
    )
}

pub(crate) fn status_failure(status: u16) -> Failure {
    match status {
        401 | 403 => Failure::Authentication,
        400 | 404 | 405 | 409 | 422 => Failure::Contract,
        413 => Failure::RequestTooLarge,
        429 => Failure::Capacity,
        503 => Failure::Unavailable,
        300..=399 => Failure::Contract,
        _ => Failure::Upstream,
    }
}
