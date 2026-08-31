use futures_util::StreamExt;
use reqwest::multipart;
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

use crate::{bounded_text, CloudAsrProviderSettings, RunCancellation};
use zeroize::Zeroizing;

const MAX_RESPONSE_BYTES: usize = 64 * 1_024;
const TRANSCRIPTION_RESPONSE_FORMAT: &str = "verbose_json";

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
    language: Option<String>,
    #[serde(default)]
    segments: Vec<TranscriptionSegment>,
}

#[derive(Debug, Deserialize)]
struct TranscriptionSegment {
    no_speech_prob: Option<f32>,
}

pub(crate) async fn probe(provider: &CloudAsrProviderSettings) -> Result<String, String> {
    let client = client(Duration::from_secs(10))?;
    let api_key = credential(provider)?;
    let mut request = client.get(operation_url(&provider.endpoint, "models")?);
    if let Some(api_key) = api_key.as_deref() {
        request = request.bearer_auth(api_key.as_str());
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Could not connect to the Cloud ASR provider".to_string())?;
    if !response.status().is_success() {
        return Err(format!("Cloud ASR returned HTTP {}", response.status()));
    }
    Ok("Cloud ASR endpoint and credential are reachable".to_string())
}

pub(crate) async fn transcribe(
    provider: &CloudAsrProviderSettings,
    samples: &[f32],
    sample_rate: u32,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<(String, Option<String>), String> {
    let wav = crate::voice::network_asr::encode_wav(samples, sample_rate)?;
    let audio = multipart::Part::bytes(wav)
        .file_name("speech.wav")
        .mime_str("audio/wav")
        .map_err(|_| "Could not encode the ASR upload".to_string())?;
    let form = multipart::Form::new()
        .part("file", audio)
        .text("model", provider.model.clone())
        .text("response_format", TRANSCRIPTION_RESPONSE_FORMAT);
    let client = client(Duration::from_millis(timeout_ms))?;
    let api_key = credential(provider)?;
    let mut request = client
        .post(operation_url(&provider.endpoint, "audio/transcriptions")?)
        .multipart(form);
    if let Some(api_key) = api_key.as_deref() {
        request = request.bearer_auth(api_key.as_str());
    }
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err("Transcription cancelled".to_string()),
        response = request.send() => response.map_err(|error| {
            if error.is_timeout() { "Cloud ASR request timed out" } else { "Cloud ASR request failed" }.to_string()
        })?,
    };
    let status = response.status();
    let body = bounded_body(response, &cancellation).await?;
    if !status.is_success() {
        return Err(format!("Cloud ASR returned HTTP {}", status.as_u16()));
    }
    let result: TranscriptionResponse = serde_json::from_slice(&body)
        .map_err(|_| "Cloud ASR returned an invalid transcription response".to_string())?;
    let text = result.text.trim();
    if response_is_no_speech(&result.segments) {
        return Err(
            "ASR_NO_SPEECH: The ASR service classified the audio as non-speech".to_string(),
        );
    }
    if text.is_empty() {
        return Err("ASR_NO_SPEECH: Cloud ASR completed without a transcript".to_string());
    }
    Ok((
        bounded_text(text, 16_000),
        result.language.map(|language| bounded_text(&language, 80)),
    ))
}

fn response_is_no_speech(segments: &[TranscriptionSegment]) -> bool {
    !segments.is_empty()
        && segments.iter().all(|segment| {
            segment
                .no_speech_prob
                .is_some_and(|probability| probability.is_finite() && probability >= 0.6)
        })
}

fn credential(
    provider: &CloudAsrProviderSettings,
) -> Result<Option<zeroize::Zeroizing<String>>, String> {
    if provider.authentication == "none" {
        return Ok(None);
    }
    crate::credentials::load_api_key(&provider.id)?
        .ok_or_else(|| "API key is not configured in macOS Keychain".to_string())
        .map(Some)
}

fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize the Cloud ASR client".to_string())
}

fn operation_url(endpoint: &str, operation: &str) -> Result<String, String> {
    let mut url =
        url::Url::parse(endpoint).map_err(|_| "Cloud ASR endpoint is invalid".to_string())?;
    let mut path = url.path().trim_end_matches('/').to_string();
    if !path.ends_with("/v1") {
        path.push_str("/v1");
    }
    path.push('/');
    path.push_str(operation);
    url.set_path(&path);
    Ok(url.to_string())
}

async fn bounded_body(
    response: reqwest::Response,
    cancellation: &RunCancellation,
) -> Result<Zeroizing<Vec<u8>>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
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
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err("Cloud ASR response exceeded the size limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_metadata_is_requested() {
        assert_eq!(TRANSCRIPTION_RESPONSE_FORMAT, "verbose_json");
    }

    #[test]
    fn rejects_only_consistently_high_no_speech_probabilities() {
        let segment = |probability| TranscriptionSegment {
            no_speech_prob: probability,
        };
        assert!(response_is_no_speech(&[
            segment(Some(0.9)),
            segment(Some(0.7))
        ]));
        assert!(!response_is_no_speech(&[
            segment(Some(0.9)),
            segment(Some(0.2))
        ]));
        assert!(!response_is_no_speech(&[segment(None)]));
        assert!(!response_is_no_speech(&[]));
    }
}
