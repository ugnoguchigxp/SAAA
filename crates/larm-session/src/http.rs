use futures_util::StreamExt;
use serde_json::Value;
pub(crate) async fn json(
    call: reqwest::RequestBuilder,
    statuses: &[u16],
) -> Result<Value, &'static str> {
    let response = call.send().await.map_err(|_| "larm_transport_failed")?;
    let status = response.status().as_u16();
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "larm_transport_failed")?;
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err("larm_response_too_large");
        }
        bytes.extend_from_slice(&chunk);
    }
    if !statuses.contains(&status) {
        let error = serde_json::from_slice::<Value>(&bytes).ok();
        let code = error
            .as_ref()
            .and_then(|value| value.pointer("/error/code"))
            .and_then(Value::as_str);
        let message = error
            .as_ref()
            .and_then(|value| value.pointer("/error/message"))
            .and_then(Value::as_str);
        return Err(match (status, code, message) {
            (400, Some("invalid_request"), Some("claim does not accept Idempotency-Key")) => {
                "larm_claim_idempotency_key_forbidden"
            }
            (400, Some("invalid_request"), Some("invalid claim request")) => {
                "larm_invalid_claim_request"
            }
            (409, Some("claim_format_mismatch"), _) => "larm_claim_format_mismatch",
            (409, Some("connection_not_ready"), _) => "larm_connection_not_ready",
            (410, Some("connection_expired"), _) => "larm_connection_expired",
            (401 | 403, _, _) => "larm_authentication_failed",
            (408, _, _) => "larm_timeout",
            (429, _, _) => "larm_capacity",
            (500..=599, _, _) => "larm_upstream_failed",
            _ => "larm_http_rejected",
        });
    }
    serde_json::from_slice(&bytes).map_err(|_| "larm_invalid_json")
}
