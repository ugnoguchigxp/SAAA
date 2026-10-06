#![cfg(test)]
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn response(bytes: Vec<u8>) -> (reqwest::Response, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route(
                "/",
                axum::routing::get(|| async move { ([("content-type", "audio/wav")], bytes) }),
            ),
        )
        .await
        .unwrap();
    });
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("http://{address}"))
        .send()
        .await
        .unwrap();
    (response, server)
}

#[tokio::test]
async fn each_chunk_notifies_once_after_audio_is_queued_without_altering_pcm() {
    let calls = Arc::new(AtomicUsize::new(0));
    for index in 0..2 {
        let wave = crate::voice::network_asr::encode_wav(&vec![0.25; 4800], 24000).unwrap();
        let mut decoder = decode::Decoder::new("wav").unwrap();
        let expected = decoder.push(&wave).unwrap();
        decoder.finish().unwrap();
        let expected_rate = decoder.format.unwrap().rate;
        let (response, server) = response(wave).await;
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<playback::Packet>(16);
        let flag = calls.clone();
        receive_with_callback(
            response,
            "wav",
            &RunCancellation::default(),
            &sender,
            &std::sync::atomic::AtomicBool::new(false),
            Some(Box::new(move || {
                assert_eq!(flag.fetch_add(1, Ordering::AcqRel), index);
            })),
        )
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::Acquire), index + 1);
        drop(sender);
        let mut actual = Vec::new();
        while let Some((format, samples)) = receiver.recv().await {
            assert_eq!(format.rate, expected_rate);
            actual.extend(samples);
        }
        assert_eq!(actual, expected);
        server.abort();
    }
}

#[tokio::test]
async fn cancelled_or_invalid_audio_never_starts_an_avatar_cue() {
    for cancel in [false, true] {
        let bytes = if cancel {
            crate::voice::network_asr::encode_wav(&[0.25; 1600], 16000).unwrap()
        } else {
            b"invalid audio".to_vec()
        };
        let (response, server) = response(bytes).await;
        let cancellation = RunCancellation::default();
        if cancel {
            cancellation.cancel();
        }
        let (sender, mut receiver) = tokio::sync::mpsc::channel(16);
        assert!(receive_with_callback(
            response,
            "wav",
            &cancellation,
            &sender,
            &std::sync::atomic::AtomicBool::new(false),
            Some(Box::new(|| panic!("no playable audio")))
        )
        .await
        .is_err());
        assert!(receiver.try_recv().is_err());
        server.abort();
    }
}
