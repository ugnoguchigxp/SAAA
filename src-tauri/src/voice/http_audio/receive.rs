//! Decode and enqueue unmodified PCM, notifying once per speech chunk.
use super::*;
pub(super) async fn receive(
    response: reqwest::Response,
    format: &str,
    cancellation: &RunCancellation,
    sender: &tokio::sync::mpsc::Sender<playback::Packet>,
    output: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    receive_with_callback(response, format, cancellation, sender, output, None).await
}

pub(super) async fn receive_with_callback(
    response: reqwest::Response,
    format: &str,
    cancellation: &RunCancellation,
    sender: &tokio::sync::mpsc::Sender<playback::Packet>,
    output: &std::sync::atomic::AtomicBool,
    mut on_ready: Option<Box<dyn FnOnce() + Send>>,
) -> Result<(), String> {
    crate::voice::cloud_tts::validate_audio_headers(&response, format)?;
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
            let format = decoder.format.ok_or_else(|| {
                "HTTP TTS streamed samples before a decoded audio format".to_string()
            })?;
            let packet_samples = (format.rate as usize / 20) * format.channels as usize;
            pending.extend(samples);
            let count = pending.len() / packet_samples * packet_samples;
            for packet in pending[..count].chunks(packet_samples) {
                output.store(true, std::sync::atomic::Ordering::Release);
                send_packet(sender, cancellation, format, packet.to_vec()).await?;
                if let Some(ready) = on_ready.take() {
                    ready();
                }
            }
            pending.drain(..count);
        }
    }
    decoder.finish()?;
    if !pending.is_empty() {
        output.store(true, std::sync::atomic::Ordering::Release);
        let format = decoder
            .format
            .ok_or_else(|| "HTTP TTS finished without a decoded audio format".to_string())?;
        send_packet(sender, cancellation, format, pending).await?;
        if let Some(ready) = on_ready.take() {
            ready();
        }
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
