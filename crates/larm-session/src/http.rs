use futures_util::StreamExt;
use serde_json::Value;
pub(crate) async fn json(
    call: reqwest::RequestBuilder,
    statuses: &[u16],
) -> Result<Value, &'static str> {
    json_response(call, statuses)
        .await
        .map(|(_, value, _, _)| value)
}

pub(crate) async fn json_response(
    call: reqwest::RequestBuilder,
    statuses: &[u16],
) -> Result<(u16, Value, Option<String>, Option<std::time::Duration>), &'static str> {
    let response = call.send().await.map_err(|_| "larm_transport_failed")?;
    let status = response.status().as_u16();
    let accepted = statuses.contains(&status);
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .parse::<u64>()
                .ok()
                .map(std::time::Duration::from_secs)
                .or_else(|| {
                    chrono::DateTime::parse_from_rfc2822(value)
                        .ok()
                        .map(|date| {
                            date.signed_duration_since(chrono::Utc::now())
                                .to_std()
                                .unwrap_or_default()
                        })
                })
        });
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "larm_transport_failed")?;
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err("larm_response_too_large");
        }
        bytes.extend_from_slice(&chunk);
    }
    if !accepted {
        let code = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| value["error"]["code"].as_str().map(str::to_string))
            .unwrap_or_default();
        return Err(classify_error(status, &code));
    }
    let value = serde_json::from_slice(&bytes).map_err(|_| "larm_invalid_json")?;
    Ok((status, value, location, retry_after))
}

fn classify_error(status: u16, code: &str) -> &'static str {
    match (status, code) {
        (409, "connection_idle_released") => "larm_connection_idle_released",
        (409, "catalog_revision_mismatch" | "revision_mismatch") => "larm_revision_mismatch",
        (409, "idempotency_conflict") => "larm_idempotency_conflict",
        (409, "provider_conflict" | "connection_audience_unavailable") => "larm_provider_conflict",
        (409, _) => "larm_conflict",
        (400, "unknown_profile" | "unknown_selector") => "larm_unknown_selector",
        (400, _) => "larm_invalid_request",
        (401 | 403, _) => "larm_authentication_failed",
        (408, _) => "larm_timeout",
        (429, _) => "larm_capacity",
        (503, _) => "larm_provider_terminal",
        (500..=599, _) => "larm_upstream_failed",
        _ => "larm_http_rejected",
    }
}

#[cfg(test)]
mod tests {
    use super::classify_error;

    #[test]
    fn maps_create_contract_conflicts_to_distinct_codes() {
        assert_eq!(
            classify_error(409, "catalog_revision_mismatch"),
            "larm_revision_mismatch"
        );
        assert_eq!(
            classify_error(409, "idempotency_conflict"),
            "larm_idempotency_conflict"
        );
        assert_eq!(
            classify_error(409, "provider_conflict"),
            "larm_provider_conflict"
        );
        assert_eq!(
            classify_error(409, "connection_audience_unavailable"),
            "larm_provider_conflict"
        );
        assert_eq!(
            classify_error(503, "provider_terminal"),
            "larm_provider_terminal"
        );
    }
}
