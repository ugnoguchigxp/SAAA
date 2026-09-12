use futures_util::StreamExt;
use reqwest::multipart;
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

use crate::{bounded_text, CloudAsrProviderSettings, RunCancellation};
use zeroize::Zeroizing;

const MAX_RESPONSE_BYTES: usize = 64 * 1_024;
const TRANSCRIPTION_RESPONSE_FORMAT: &str = "json";

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
    let samples: Vec<f32> = (0..1600)
        .map(|index| ((index as f32 * 440.0 * std::f32::consts::TAU / 16000.0).sin()) * 0.1)
        .collect();
    match transcribe(provider, &samples, 16_000, 10_000, Arc::default()).await {
        Ok(_) => Ok("HTTP ASR accepted a fixed WAV upload".to_string()),
        Err(error) if error.starts_with("ASR_NO_SPEECH:") => {
            Ok("HTTP ASR accepted a fixed WAV upload (no speech)".to_string())
        }
        Err(error) => Err(error),
    }
}

pub(crate) async fn transcribe(
    provider: &CloudAsrProviderSettings,
    samples: &[f32],
    sample_rate: u32,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
) -> Result<(String, Option<String>), String> {
    transcribe_with_api_key(
        provider,
        samples,
        sample_rate,
        timeout_ms,
        cancellation,
        None,
    )
    .await
}

pub(crate) async fn transcribe_with_api_key(
    provider: &CloudAsrProviderSettings,
    samples: &[f32],
    sample_rate: u32,
    timeout_ms: u64,
    cancellation: Arc<RunCancellation>,
    claim_key: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let request_started = std::time::Instant::now();
    if cancellation.is_cancelled() {
        return Err("Transcription cancelled".into());
    }
    if samples.len() > sample_rate as usize * 600 {
        return Err("ASR audio exceeds ten minutes".into());
    }
    let wav = crate::voice::network_asr::encode_wav(samples, sample_rate)?;
    if samples.iter().all(|sample| sample.abs() <= f32::EPSILON) {
        return Err("ASR_NO_SPEECH: The audio is silent".into());
    }
    let client = client(Duration::from_millis(timeout_ms), claim_key.is_some())?;
    let configured_key = if claim_key.is_none() {
        credential(provider)?
    } else {
        None
    };
    let api_key = claim_key.or(configured_key.as_deref().map(String::as_str));
    let url = operation_url(&provider.endpoint, "audio/transcriptions")?;
    let operation = async {
        let response = crate::providers::http::send_with(
            || {
                let audio = multipart::Part::bytes(wav.clone())
                    .file_name("speech.wav")
                    .mime_str("audio/wav")
                    .map_err(|_| crate::providers::stream::ProviderFailureKind::Contract)?;
                let form = multipart::Form::new()
                    .part("file", audio)
                    .text("model", provider.model.clone())
                    .text("response_format", TRANSCRIPTION_RESPONSE_FORMAT);
                let mut request = client.post(&url).multipart(form);
                if let Some(api_key) = api_key {
                    request = request.bearer_auth(api_key);
                }
                Ok(request)
            },
            &cancellation,
            true,
        )
        .await
        .map_err(|kind| kind.public_message().as_str().to_string())?;
        bounded_body(response, &cancellation).await
    };
    let body = tokio::time::timeout(Duration::from_millis(timeout_ms), operation)
        .await
        .map_err(|_| "HTTP ASR request timed out".to_string())??;
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
    crate::providers::http_metrics::record("asrUploadToFinalText", request_started.elapsed());
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

fn client(timeout: Duration, claim_scoped: bool) -> Result<reqwest::Client, String> {
    super::http_audio::client::build(
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none()),
        claim_scoped,
    )
}

fn operation_url(endpoint: &str, operation: &str) -> Result<String, String> {
    crate::providers::openai_compatible::provider_operation_url(endpoint, operation)
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
    fn text_only_json_is_supported() {
        assert_eq!(TRANSCRIPTION_RESPONSE_FORMAT, "json");
        let result: TranscriptionResponse = serde_json::from_str(r#"{"text":"日本語"}"#).unwrap();
        assert_eq!(result.text, "日本語");
        assert!(result.language.is_none());
        assert!(!response_is_no_speech(&result.segments));
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
