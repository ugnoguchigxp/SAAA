//! Explicit operator test: uses synthetic speech and the real output device, never the microphone.
use super::{playback, receive};
use crate::{CloudAsrProviderSettings, CloudTtsProviderSettings, RunCancellation};
use std::{sync::Arc, time::Instant};

#[tokio::test]
#[ignore = "operator-only HTTP TTS playback and synthetic-speech ASR; requires SAAA_HTTP_LIVE_BASE_URL and LARM_API_TOKEN"]
async fn live_tts_playback_and_asr() {
    let endpoint =
        std::env::var("SAAA_HTTP_LIVE_BASE_URL").expect("explicit live endpoint required");
    let key = zeroize::Zeroizing::new(
        std::env::var("LARM_API_TOKEN").expect("explicit live API credential required"),
    );
    let tts = CloudTtsProviderSettings {
        id: "http-live-test".into(),
        enabled: true,
        label: "HTTP live test".into(),
        location: "local".into(),
        endpoint: endpoint.clone(),
        model: "voicevox-core".into(),
        voice: "Kasukabe_Tsumugi".into(),
        authentication: "api-key".into(),
        response_format: "wav".into(),
    };
    let cancellation = Arc::new(RunCancellation::default());
    let started = Instant::now();
    let (start_tx, start_rx) = tokio::sync::oneshot::channel();
    let response = crate::voice::cloud_tts::request_audio_with_api_key(
        &tts,
        "疎通確認です。",
        30_000,
        cancellation.clone(),
        Some(&key),
    )
    .await
    .expect("HTTP TTS request succeeds");
    let player = playback::Playback::start(cancellation.clone(), move || {
        let _ = start_tx.send(started.elapsed());
    });
    let output = player.sender.clone();
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<playback::Packet>(4);
    let collector = tokio::spawn(async move {
        let mut samples = Vec::new();
        while let Some((format, packet)) = receiver.recv().await {
            assert_eq!(format.channels, 1, "live fixture requires mono TTS");
            assert_eq!(format.rate, 24_000, "live fixture requires 24 kHz TTS");
            samples.extend(packet.iter().map(|sample| f32::from(*sample) / 32768.0));
            assert!(samples.len() <= 24_000 * 30, "synthetic speech is bounded");
            output
                .send((format, packet))
                .await
                .expect("player receives audio");
        }
        samples
    });
    let received = receive(response, "wav", &cancellation, &sender).await;
    drop(sender);
    let samples = collector.await.expect("audio collection completes");
    received.expect("SAAA incrementally decodes HTTP audio");
    player
        .finish()
        .await
        .expect("SAAA native playback completes");
    let first_sample = start_rx.await.expect("native mixer consumes audio");
    eprintln!(
        "live_tts: native_playback_complete=true first_mixer_sample_ms={} audio_samples={}",
        first_sample.as_millis(),
        samples.len()
    );
    assert!(!samples.is_empty());
    let asr = CloudAsrProviderSettings {
        id: "http-live-test-asr".into(),
        enabled: true,
        label: "HTTP live test ASR".into(),
        location: "local".into(),
        endpoint,
        model: "qwen3-asr-1.7b".into(),
        language: "auto".into(),
        authentication: "api-key".into(),
    };
    let asr_started = Instant::now();
    let (text, _) = crate::voice::cloud_asr::transcribe_with_api_key(
        &asr,
        &samples,
        24_000,
        30_000,
        cancellation,
        Some(&key),
    )
    .await
    .expect("SAAA transcribes synthetic Japanese speech");
    assert!(!text.trim().is_empty());
    eprintln!(
        "live_asr: nonempty_transcription=true upload_to_text_ms={}",
        asr_started.elapsed().as_millis()
    );
}
