use futures_util::StreamExt;
use serde_json::Value;
pub(crate) async fn json(
    call: reqwest::RequestBuilder,
    statuses: &[u16],
) -> Result<Value, &'static str> {
    let response = call.send().await.map_err(|_| "larm_transport_failed")?;
    if !statuses.contains(&response.status().as_u16()) {
        return Err(match response.status().as_u16() {
            401 | 403 => "larm_authentication_failed",
            408 => "larm_timeout",
            429 => "larm_capacity",
            500..=599 => "larm_upstream_failed",
            _ => "larm_http_rejected",
        });
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "larm_transport_failed")?;
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err("larm_response_too_large");
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "larm_invalid_json")
}
