use crate::{CloudTtsProviderSettings, RunCancellation};
use futures_util::StreamExt;
use std::sync::Arc;
pub(crate) mod client;
pub(crate) mod decode;
#[cfg(all(test, not(coverage)))]
mod live;
mod playback;
mod timeout_tests;

mod requests;
pub(crate) use requests::{play_larm_with_situation, play_with_situation};

#[allow(clippy::too_many_arguments)]
async fn play_response(
    response: reqwest::Response,
    format: &str,
    cancellation: Arc<RunCancellation>,
    on_started: impl FnOnce() + Send + 'static,
    started: std::time::Instant,
    lease: Option<(saaa_larm_session::Use, std::time::Duration)>,
    output: Arc<std::sync::atomic::AtomicBool>,
    situation: Option<Arc<crate::situation::SituationRuntime>>,
) -> Result<(), String> {
    if situation
        .as_ref()
        .is_some_and(|s| crate::situation::speech_holds_runtime(s))
        || cancellation.is_cancelled()
    {
        return Ok(());
    }
    let player = playback::Playback::start_guarded(cancellation.clone(), situation, move || {
        crate::providers::http_metrics::record("ttsRequestToFirstMixerSample", started.elapsed());
        on_started();
    });
    if let Some((_, remaining)) = &lease {
        receive_with_timeout(
            response,
            format,
            &cancellation,
            &player.sender,
            *remaining,
            &output,
        )
        .await?;
    } else {
        receive(response, format, &cancellation, &player.sender, &output).await?;
    }
    // Provider I/O is complete. Playback must not hold up token renewal.
    drop(lease);
    tokio::select! {
        biased;
        _=cancellation.cancelled()=>Err("Speech cancelled".into()),
        result=player.finish()=>result,
    }
}

async fn receive_with_timeout(
    response: reqwest::Response,
    format: &str,
    cancellation: &RunCancellation,
    sender: &tokio::sync::mpsc::Sender<playback::Packet>,
    remaining: std::time::Duration,
    output: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    // reqwest's timeout cannot interrupt waiting for the audio output queue.
    tokio::time::timeout(
        remaining,
        receive(response, format, cancellation, sender, output),
    )
    .await
    .map_err(|_| "TTS audio receive timed out")?
}

async fn receive(
    response: reqwest::Response,
    format: &str,
    cancellation: &RunCancellation,
    sender: &tokio::sync::mpsc::Sender<playback::Packet>,
    output: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    super::cloud_tts::validate_audio_headers(&response, format)?;
    let mut decoder = decode::Decoder::new(format)?;
    let received_headers = std::time::Instant::now();
    let mut first_audio = true;
    let mut pending = Vec::new();
    let mut stream = response.bytes_stream();
    let mut bytes = 0;
    loop {
        let chunk = tokio::select! {biased; _=cancellation.cancelled()=>return Err("Speech cancelled".into()), chunk=stream.next()=>chunk};
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| "HTTP TTS audio was interrupted".to_string())?;
        bytes += chunk.len();
        if bytes > 16 * 1024 * 1024 {
            return Err("TTS audio exceeded the size limit".into());
        }
        let samples = decoder.push(&chunk)?;
        if !samples.is_empty() {
            if first_audio {
                crate::providers::http_metrics::record(
                    "ttsHeadersToFirstDecodableAudio",
                    received_headers.elapsed(),
                );
                first_audio = false;
            }
            let format = decoder.format.unwrap();
            let packet_samples = (format.rate as usize / 20) * format.channels as usize;
            pending.extend(samples);
            let count = pending.len() / packet_samples * packet_samples;
            for packet in pending[..count].chunks(packet_samples) {
                output.store(true, std::sync::atomic::Ordering::Release);
                send_packet(sender, cancellation, format, packet.to_vec()).await?;
            }
            pending.drain(..count);
        }
    }
    decoder.finish()?;
    if !pending.is_empty() {
        output.store(true, std::sync::atomic::Ordering::Release);
        send_packet(sender, cancellation, decoder.format.unwrap(), pending).await?;
    }
    Ok(())
}

async fn send_packet(
    sender: &tokio::sync::mpsc::Sender<playback::Packet>,
    cancellation: &RunCancellation,
    format: decode::Format,
    samples: Vec<i16>,
) -> Result<(), String> {
    tokio::select! {biased;
        _=cancellation.cancelled()=>Err("Speech cancelled".into()),
        result=sender.send((format,samples))=>result.map_err(|_|"Audio output worker disconnected".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let n = socket.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buffer[..n]);
            if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..end]);
                let length = header.lines().find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|v| v.parse::<usize>().ok())
                });
                if length.is_some_and(|length| bytes.len() >= end + 4 + length) {
                    return bytes;
                }
                // Multipart may use chunked encoding.
                if header
                    .to_ascii_lowercase()
                    .contains("transfer-encoding: chunked")
                    && bytes.ends_with(b"0\r\n\r\n")
                {
                    return bytes;
                }
            }
        }
    }
    fn tts(endpoint: String, format: &str) -> CloudTtsProviderSettings {
        CloudTtsProviderSettings {
            id: "fixture".into(),
            enabled: true,
            label: "fixture".into(),
            location: "local".into(),
            endpoint,
            model: "voice-model".into(),
            voice: "voice".into(),
            authentication: "none".into(),
            response_format: format.into(),
        }
    }

    #[tokio::test]
    async fn wav_and_pcm_are_playable_before_the_rest_of_the_body_arrives() {
        for format in ["wav", "pcm"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!(
                "http://{}/proxy/v1/audio/speech",
                listener.local_addr().unwrap()
            );
            let bytes = if format == "wav" {
                crate::voice::network_asr::encode_wav(&[0.1; 1600], 16000).unwrap()
            } else {
                vec![0; 4800]
            };
            let (gate, wait) = tokio::sync::oneshot::channel();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let request = request(&mut socket).await;
                let request = String::from_utf8(request).unwrap();
                assert!(request.starts_with("POST /proxy/v1/audio/speech HTTP/1.1"));
                if format == "pcm" {
                    assert!(request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer ephemeral-audio"));
                }
                assert!(!request.contains("\"stream\""));
                assert!(request.contains("\"voice\":\"voice\""));
                let header=format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len());
                socket.write_all(header.as_bytes()).await.unwrap();
                let split = (bytes.len() / 2 + 44).min(bytes.len() - 2);
                socket.write_all(&bytes[..split]).await.unwrap();
                wait.await.unwrap();
                socket.write_all(&bytes[split..]).await.unwrap();
            });
            let cancellation = Arc::new(RunCancellation::default());
            let mut provider = tts(endpoint, format);
            if format == "pcm" {
                provider.authentication = "api-key".into();
            }
            let response = super::super::cloud_tts::request_audio_with_api_key(
                &provider,
                "短いテストです。",
                5000,
                cancellation.clone(),
                (format == "pcm").then_some("ephemeral-audio"),
            )
            .await
            .unwrap();
            let (sender, mut receiver) = tokio::sync::mpsc::channel::<playback::Packet>(4);
            let consumer = tokio::spawn(async move {
                let first =
                    tokio::time::timeout(std::time::Duration::from_secs(1), receiver.recv())
                        .await
                        .unwrap()
                        .unwrap();
                assert!(!first.1.is_empty());
                gate.send(()).unwrap();
                while receiver.recv().await.is_some() {}
            });
            receive(
                response,
                format,
                &cancellation,
                &sender,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .await
            .unwrap();
            drop(sender);
            consumer.await.unwrap();
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn buffered_wav_keeps_artifact_support() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let wave = crate::voice::network_asr::encode_wav(&[0.1; 160], 16000).unwrap();
        let expected = wave.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\n\r\n",
                wave.len()
            );
            socket.write_all(header.as_bytes()).await.unwrap();
            socket.write_all(&wave).await.unwrap();
        });
        let directory = tempfile::tempdir().unwrap();
        let artifact = super::super::cloud_tts::render_to_artifact(
            &tts(endpoint, "wav"),
            "test",
            5000,
            Arc::default(),
            directory.path(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(artifact).unwrap(), expected);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn asr_upload_accepts_text_only_json_and_no_speech() {
        for text in ["日本語", ""] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!(
                "http://{}/proxy/v1/audio/transcriptions",
                listener.local_addr().unwrap()
            );
            let body = serde_json::json!({"text":text}).to_string();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let bytes = request(&mut socket).await;
                let request = String::from_utf8_lossy(&bytes);
                assert!(request.starts_with("POST /proxy/v1/audio/transcriptions HTTP/1.1"));
                if text.is_empty() {
                    assert!(request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer ephemeral-asr"));
                }
                assert!(request.contains("name=\"file\""));
                assert!(request.contains("name=\"model\""));
                assert!(bytes.windows(4).any(|v| v == b"RIFF"));
                assert!(!request.contains("verbose_json"));
                let response=format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len());
                socket.write_all(response.as_bytes()).await.unwrap();
            });
            let provider = crate::CloudAsrProviderSettings {
                id: "asr".into(),
                enabled: true,
                label: "asr".into(),
                location: "local".into(),
                endpoint,
                model: "asr-model".into(),
                language: "auto".into(),
                authentication: if text.is_empty() { "api-key" } else { "none" }.into(),
            };
            let result = super::super::cloud_asr::transcribe_with_api_key(
                &provider,
                &[0.1; 1600],
                16000,
                5000,
                Arc::default(),
                text.is_empty().then_some("ephemeral-asr"),
            )
            .await;
            if text.is_empty() {
                assert!(result.unwrap_err().starts_with("ASR_NO_SPEECH:"));
            } else {
                assert_eq!(result.unwrap(), (text.into(), None));
            }
            assert!(super::super::cloud_asr::transcribe(
                &provider,
                &[0.0; 1600],
                16000,
                5000,
                Arc::default()
            )
            .await
            .unwrap_err()
            .starts_with("ASR_NO_SPEECH:"));
            server.await.unwrap();
        }
    }
}
