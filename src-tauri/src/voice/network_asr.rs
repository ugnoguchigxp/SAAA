use crate::{bounded_text, AppState, RunCancellation};
use futures_util::StreamExt;
use reqwest::{multipart, Client, Response};
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

mod discovery;
mod runtime;
pub(crate) use discovery::base_url_from_host;
use discovery::ensure_selected_model;
pub use discovery::probe;
#[cfg(test)]
use discovery::{resolve_at, validate_base_url};
pub(crate) use runtime::NetworkAsrRuntime;

pub const PROVIDER_ID: &str = "network-asr";
pub const MODEL_ID: &str = "qwen3-asr-1.7b";
pub const DEFAULT_HOST: &str = "localhost";

const MAX_RESPONSE_BYTES: usize = 64 * 1_024;
const MAX_TRANSCRIPT_CHARS: usize = 16_000;

#[derive(Debug, Deserialize)]
struct TranscriptionResponse {
    text: String,
    language: Option<String>,
}

pub async fn transcribe(
    state: &AppState,
    samples: &[f32],
    sample_rate: u32,
    model: &str,
    cancellation: Arc<RunCancellation>,
) -> Result<(String, Option<String>), String> {
    let host = discovery::configured_host(state)?;
    let resolution = state
        .network_asr
        .resolve(&host, cancellation.clone())
        .await?;
    ensure_selected_model(&resolution, model)?;
    let result = transcribe_at(
        state.network_asr.client(),
        &resolution.endpoint,
        samples,
        sample_rate,
        model,
        cancellation.clone(),
    )
    .await;
    if result.is_err() && !cancellation.is_cancelled() {
        state.network_asr.invalidate(&host).await;
    }
    result
}

async fn transcribe_at(
    client: &Client,
    base_url: &str,
    samples: &[f32],
    sample_rate: u32,
    model: &str,
    cancellation: Arc<RunCancellation>,
) -> Result<(String, Option<String>), String> {
    let wav = encode_wav(samples, sample_rate)?;
    let audio = multipart::Part::bytes(wav)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|_| "Could not encode the ASR upload".to_string())?;
    let form = multipart::Form::new()
        .part("file", audio)
        .text("model", model.to_string())
        .text("language", "auto")
        .text("response_format", "json");
    let request = client
        .post(format!(
            "{}/v1/audio/transcriptions",
            base_url.trim_end_matches('/')
        ))
        .multipart(form)
        .send();
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err("Transcription cancelled".to_string()),
        response = request => response.map_err(|error| format!("Could not reach LAN ASR: {error}"))?,
    };
    let status = response.status();
    let body = bounded_response(response, &cancellation).await?;
    if !status.is_success() {
        return Err(format!("LAN ASR returned HTTP {}", status.as_u16()));
    }
    let result: TranscriptionResponse = serde_json::from_slice(&body)
        .map_err(|_| "LAN ASR returned an invalid transcription response".to_string())?;
    let text = result.text.trim();
    if text.is_empty() {
        return Err("LAN ASR completed without a transcript".to_string());
    }
    Ok((
        bounded_text(text, MAX_TRANSCRIPT_CHARS),
        result.language.map(|language| bounded_text(&language, 80)),
    ))
}

pub(super) fn client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| format!("Could not initialize LAN ASR client: {error}"))
}

pub(super) async fn bounded_response(
    response: Response,
    cancellation: &RunCancellation,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err("LAN ASR response exceeded the size limit".to_string());
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err("Transcription cancelled".to_string()),
            chunk = stream.next() => chunk,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(|error| format!("Could not read LAN ASR response: {error}"))?;
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err("LAN ASR response exceeded the size limit".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, String> {
    if samples.is_empty()
        || !(8_000..=192_000).contains(&sample_rate)
        || samples.iter().any(|sample| !sample.is_finite())
    {
        return Err("Invalid audio samples".to_string());
    }
    let resampled = resample_pcm(samples, sample_rate, 16_000);
    let data_size = resampled
        .len()
        .checked_mul(2)
        .and_then(|size| u32::try_from(size).ok())
        .ok_or_else(|| "Recorded audio is too large".to_string())?;
    let mut wav = Vec::with_capacity(44 + data_size as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36_u32 + data_size).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&16_000_u32.to_le_bytes());
    wav.extend_from_slice(&32_000_u32.to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());
    for sample in resampled {
        let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        wav.extend_from_slice(&pcm.to_le_bytes());
    }
    Ok(wav)
}

pub fn resample_pcm(samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
    if samples.is_empty() || source_rate == 0 || target_rate == 0 {
        return Vec::new();
    }
    if source_rate == target_rate {
        return samples.to_vec();
    }
    let ratio = source_rate as f64 / target_rate as f64;
    let target_len = ((samples.len() as f64) / ratio).floor() as usize;
    (0..target_len)
        .map(|index| {
            let position = index as f64 * ratio;
            let left = position.floor() as usize;
            let right = (left + 1).min(samples.len().saturating_sub(1));
            let fraction = (position - left as f64) as f32;
            samples[left] * (1.0 - fraction) + samples[right] * fraction
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn asr_base_url_accepts_only_private_http_origins() {
        assert_eq!(
            validate_base_url("http://10.0.0.42:8081/").expect("private origin is accepted"),
            "http://10.0.0.42:8081"
        );
        assert!(validate_base_url("http://llm-server.local:8081").is_ok());
        for invalid in [
            "https://10.0.0.42:8081",
            "http://example.com:8081",
            "http://user:secret@10.0.0.42:8081",
            "http://10.0.0.42:8081/v1",
        ] {
            assert!(validate_base_url(invalid).is_err(), "accepted {invalid}");
        }
        assert_eq!(
            base_url_from_host("10.0.0.42").expect("host derives"),
            "http://10.0.0.42:8081"
        );
        assert!(base_url_from_host("http://10.0.0.42").is_err());
    }

    #[tokio::test]
    async fn resolves_endpoint_and_model_from_asr_apis() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            for (path, body) in [
                (
                    "/v1/models",
                    r#"{"object":"list","data":[{"id":"qwen3-asr-1.7b"}]}"#,
                ),
                ("/health", r#"{"status":"ok","model":"qwen3-asr-1.7b"}"#),
            ] {
                let (mut socket, _) = listener.accept().expect("fixture accepts");
                let mut request = [0_u8; 4_096];
                let count = socket.read(&mut request).expect("fixture reads");
                let request = String::from_utf8_lossy(&request[..count]);
                assert!(request.starts_with(&format!("GET {path} HTTP/1.1")));
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("fixture responds");
            }
        });

        let resolution = resolve_at(&format!("http://{address}"))
            .await
            .expect("settings resolve");
        server.join().expect("fixture joins");
        assert_eq!(resolution.provider_id, PROVIDER_ID);
        assert_eq!(resolution.endpoint, format!("http://{address}"));
        assert_eq!(resolution.model, MODEL_ID);
    }

    #[test]
    fn wav_encoding_is_mono_pcm_at_sixteen_kilohertz() {
        let wav = encode_wav(&[0.0; 8_000], 8_000).expect("wav encodes");
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 32_000);
        assert_eq!(wav.len(), 32_044);
    }

    #[tokio::test]
    async fn openai_compatible_multipart_contract_returns_bounded_transcript() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("fixture accepts");
            let mut request = Vec::new();
            let mut expected_len = None;
            loop {
                let mut chunk = [0_u8; 4_096];
                let count = socket.read(&mut chunk).expect("fixture reads");
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                if expected_len.is_none() {
                    if let Some(header_end) =
                        request.windows(4).position(|part| part == b"\r\n\r\n")
                    {
                        let headers = String::from_utf8_lossy(&request[..header_end]);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length: ")
                                    .or_else(|| line.strip_prefix("Content-Length: "))
                            })
                            .and_then(|value| value.parse::<usize>().ok())
                            .expect("multipart request has a content length");
                        expected_len = Some(header_end + 4 + content_length);
                    }
                }
                if expected_len.is_some_and(|length| request.len() >= length) {
                    break;
                }
            }
            let request_text = String::from_utf8_lossy(&request);
            assert!(request_text.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
            assert!(request_text.contains("name=\"model\""));
            assert!(request_text.contains(MODEL_ID));
            assert!(request_text.contains("name=\"language\""));
            assert!(request_text.contains("audio/wav"));

            let body = r#"{"text":"dynamic_lan transcript","language":"Japanese"}"#;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("fixture responds");
        });

        let result = transcribe_at(
            &client().expect("ASR client initializes"),
            &format!("http://{address}"),
            &[0.0; 8_000],
            8_000,
            MODEL_ID,
            Arc::new(RunCancellation::default()),
        )
        .await
        .expect("transcription succeeds");
        server.join().expect("fixture joins");
        assert_eq!(result.0, "dynamic_lan transcript");
        assert_eq!(result.1.as_deref(), Some("Japanese"));
    }

    #[tokio::test]
    async fn response_body_read_stops_when_transcription_is_cancelled() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("fixture accepts");
            let mut request = [0_u8; 4_096];
            let _ = socket.read(&mut request).expect("fixture reads request");
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 10\r\nConnection: close\r\n\r\n",
                )
                .expect("fixture writes headers");
            thread::sleep(Duration::from_millis(200));
        });
        let response = client()
            .expect("client builds")
            .get(format!("http://{address}/health"))
            .send()
            .await
            .expect("headers arrive");
        let cancellation = Arc::new(RunCancellation::default());
        let cancellation_for_task = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancellation_for_task.cancel();
        });

        assert_eq!(
            bounded_response(response, &cancellation).await,
            Err("Transcription cancelled".to_string())
        );
        server.join().expect("fixture joins");
    }
}
