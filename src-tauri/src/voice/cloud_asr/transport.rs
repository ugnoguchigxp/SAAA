use super::*;

pub(super) fn credential(
    provider: &CloudAsrProviderSettings,
) -> Result<Option<zeroize::Zeroizing<String>>, String> {
    if provider.authentication == "none" {
        return Ok(None);
    }
    crate::credentials::load_api_key(&provider.id)?
        .ok_or_else(|| {
            "API key is not configured in the operating system credential store".to_string()
        })
        .map(Some)
}

pub(super) fn client(timeout: Duration, claim_scoped: bool) -> Result<reqwest::Client, String> {
    crate::voice::http_audio::client::build(
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none()),
        claim_scoped,
    )
}

pub(super) fn operation_url(endpoint: &str, operation: &str) -> Result<String, String> {
    crate::providers::openai_compatible::provider_operation_url(endpoint, operation)
}

pub(super) async fn bounded_body(
    response: reqwest::Response,
    cancellation: &RunCancellation,
    max_response_bytes: usize,
) -> Result<Zeroizing<Vec<u8>>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > max_response_bytes as u64)
    {
        return Err("Cloud ASR response exceeded the size limit".to_string());
    }
    let mut stream = response.bytes_stream();
    let mut body = Zeroizing::new(Vec::new());
    loop {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err("Transcription cancelled".to_string()),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| "Cloud ASR response was interrupted".to_string())?;
        if body.len().saturating_add(chunk.len()) > max_response_bytes {
            return Err("Cloud ASR response exceeded the size limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
