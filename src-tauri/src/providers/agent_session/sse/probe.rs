use super::*;

pub(in crate::providers::agent_session) async fn probe_event_stream(
    client: &Client,
    events_url: &Url,
    api_key: Option<&str>,
) -> Result<(), String> {
    let response = authorized(
        client
            .get(events_url.clone())
            .header(header::ACCEPT, "text/event-stream"),
        api_key,
    )
    .send()
    .await
    .map_err(|error| format!("Connection failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Provider returned HTTP {}", response.status()));
    }
    if !is_event_stream(&response) {
        return Err("Agent Session events endpoint did not return text/event-stream".to_string());
    }
    Ok(())
}
