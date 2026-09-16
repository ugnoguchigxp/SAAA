#![cfg(test)]
use super::*;
use std::time::Duration;

#[tokio::test]
async fn stalled_audio_output_cannot_hold_the_provider_lease_indefinitely() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let wave = crate::voice::network_asr::encode_wav(&[0.1; 16000], 16000).unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/",
                axum::routing::get(|| async move { ([("content-type", "audio/wav")], wave) }),
            ),
        )
        .await
        .unwrap();
    });
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap();
    // Keep the receiver alive without consuming: the mixer has stopped progressing.
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let output = std::sync::atomic::AtomicBool::new(false);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        receive_with_timeout(
            response,
            "wav",
            &RunCancellation::default(),
            &sender,
            Duration::from_millis(50),
            &output,
        ),
    )
    .await
    .unwrap();
    assert_eq!(result.unwrap_err(), "TTS audio receive timed out");
    assert!(receiver.try_recv().is_ok());
    assert!(
        output.load(std::sync::atomic::Ordering::Acquire),
        "queued audio must prohibit replay through a fallback even before the mixer consumes it"
    );
    server.abort();
}
