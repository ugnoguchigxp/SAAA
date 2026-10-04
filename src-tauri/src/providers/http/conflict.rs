use super::*;

pub(super) async fn failure(response: reqwest::Response) -> Failure {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    use futures_util::StreamExt;
    while let Some(part) = stream.next().await {
        let part = match part {
            Ok(part) => part,
            Err(_) => return Failure::Network,
        };
        if body.len() + part.len() > 4096 {
            return Failure::Contract;
        }
        body.extend_from_slice(&part);
    }
    if serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .is_some_and(|value| value["error"]["code"] == "connection_idle_released")
    {
        return Failure::AllocationLost;
    }
    Failure::Contract
}
