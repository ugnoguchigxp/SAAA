//! Shared conservative fallback decisions for adapters with public string errors.
use super::stream::ProviderFailureKind as Kind;
pub(crate) fn failure_kind(error: &str) -> Kind {
    for kind in [
        Kind::Authentication,
        Kind::Contract,
        Kind::Protocol,
        Kind::RequestTooLarge,
        Kind::Policy,
        Kind::Capacity,
        Kind::Unavailable,
        Kind::Upstream,
        Kind::Network,
        Kind::Timeout,
        Kind::Cancelled,
    ] {
        if error == kind.public_message().as_str() {
            return kind;
        }
    }
    if let Some(rest) = error.split(" returned HTTP ").nth(1) {
        return match rest
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u16>().ok())
        {
            Some(401 | 403) => Kind::Authentication,
            Some(408) => Kind::Timeout,
            Some(429) => Kind::Capacity,
            Some(500..=599) => Kind::Upstream,
            _ => Kind::Contract,
        };
    }
    if error.starts_with("LAN ASR request timed out.") {
        return Kind::Timeout;
    }
    if error.starts_with("Could not connect to LAN ASR.")
        || error.starts_with("LAN ASR request failed.")
    {
        return Kind::Network;
    }
    match error {
        "larm_expired" | "larm_session_closed" => Kind::AllocationLost,
        "larm_transport_failed" => Kind::Network,
        "larm_timeout" => Kind::Timeout,
        "larm_capacity" => Kind::Capacity,
        "larm_upstream_failed" => Kind::Upstream,
        "larm_authentication_failed" => Kind::Authentication,
        "larm_unknown_provider" | "larm_unhealthy_provider" | "larm_session_unavailable" => {
            Kind::Unavailable
        }
        "Could not connect to the Provider Harness"
        | "Provider Harness response was interrupted"
        | "Cloud ASR response was interrupted"
        | "Cloud TTS response was interrupted"
        | "HTTP TTS audio was interrupted" => Kind::Network,
        "HTTP ASR request timed out"
        | "ASR request reached its configured timeout"
        | "TTS audio receive timed out"
        | "TTS request timed out"
        | "LARM ASR timed out"
        | "LAN ASR request timed out" => Kind::Timeout,
        "larm_provider_unavailable" | "asr-provider-unavailable" => Kind::Unavailable,
        _ => Kind::Contract,
    }
}
pub(crate) fn retryable(error: &str) -> bool {
    matches!(
        failure_kind(error),
        Kind::Capacity
            | Kind::Unavailable
            | Kind::Upstream
            | Kind::Network
            | Kind::Timeout
            | Kind::AllocationLost
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_transient_failures_allow_fallback() {
        for error in [
            "LAN ASR returned HTTP 503",
            "Provider Harness returned HTTP 429 Too Many Requests",
            "HTTP ASR request timed out",
        ] {
            assert!(retryable(error), "{error}");
        }
        for error in [
            "LAN ASR returned HTTP 401",
            "Provider Harness returned HTTP 422",
            "invalid descriptor",
            "ASR_NO_SPEECH",
            "Speech cancelled",
        ] {
            assert!(!retryable(error), "{error}");
        }
    }
}
