use std::time::Duration;

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
use zeroize::Zeroizing;

use super::harness_stream::{self, ClientEvent, ProviderEvent, STREAM_PROTOCOL};
use crate::providers::service_harness::AsrStreamingDescriptor;

const SEND_TIMEOUT: Duration = Duration::from_secs(1);
const READY_TIMEOUT: Duration = Duration::from_secs(5);
const EVENT_CAPACITY: usize = 16;

pub(crate) enum NativeInbound {
    Provider(ProviderEvent),
    Ping(Vec<u8>),
    Closed(&'static str),
}

#[async_trait]
pub(crate) trait NativeAsrSink: Send {
    async fn audio(&mut self, bytes: Zeroizing<Vec<u8>>) -> Result<(), String>;
    async fn commit(
        &mut self,
        utterance_id: &str,
        next_utterance_id: &str,
        end_sample: u64,
    ) -> Result<(), String>;
    async fn stop(&mut self, session_id: &str) -> Result<(), String>;
    async fn pong(&mut self, payload: Vec<u8>) -> Result<(), String>;
}

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type SocketWriter = futures_util::stream::SplitSink<Socket, Message>;

struct WebSocketSink {
    writer: SocketWriter,
}

#[async_trait]
impl NativeAsrSink for WebSocketSink {
    async fn audio(&mut self, bytes: Zeroizing<Vec<u8>>) -> Result<(), String> {
        let message = harness_stream::audio_message(bytes.to_vec())?;
        send_with_timeout(&mut self.writer, message).await
    }

    async fn commit(
        &mut self,
        utterance_id: &str,
        next_utterance_id: &str,
        end_sample: u64,
    ) -> Result<(), String> {
        let text = harness_stream::encode_client_event(&ClientEvent::Commit {
            utterance_id,
            next_utterance_id,
            end_sample,
        })?;
        send_with_timeout(&mut self.writer, Message::Text(text.into())).await
    }

    async fn stop(&mut self, session_id: &str) -> Result<(), String> {
        let text = harness_stream::encode_client_event(&ClientEvent::Stop { session_id })?;
        send_with_timeout(&mut self.writer, Message::Text(text.into())).await
    }

    async fn pong(&mut self, payload: Vec<u8>) -> Result<(), String> {
        send_with_timeout(&mut self.writer, Message::Pong(payload.into())).await
    }
}

pub(crate) struct NativeConnection {
    pub(crate) sink: Box<dyn NativeAsrSink>,
    pub(crate) events: mpsc::Receiver<NativeInbound>,
    reader: tokio::task::JoinHandle<()>,
}

impl Drop for NativeConnection {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

pub(crate) async fn open(
    descriptor: &AsrStreamingDescriptor,
    session_id: &str,
    utterance_id: &str,
    model: &str,
    language: &str,
) -> Result<NativeConnection, String> {
    let mut socket = harness_stream::connect(descriptor).await?;
    let start = harness_stream::encode_client_event(&ClientEvent::Start {
        contract_version: STREAM_PROTOCOL,
        session_id,
        utterance_id,
        model,
        sample_rate: 16_000,
        encoding: "pcm_s16le",
        packet_milliseconds: 100,
        language,
    })?;
    send_socket_with_timeout(&mut socket, Message::Text(start.into())).await?;
    wait_ready(&mut socket, session_id, utterance_id).await?;

    let (writer, mut reader) = socket.split();
    let (event_tx, event_rx) = mpsc::channel(EVENT_CAPACITY);
    let reader_task = tokio::spawn(async move {
        loop {
            let inbound = match reader.next().await {
                Some(Ok(Message::Text(text))) => {
                    match harness_stream::decode_provider_event(text.as_bytes()) {
                        Ok(event) => NativeInbound::Provider(event),
                        Err(_) => NativeInbound::Closed("asr-stream-protocol"),
                    }
                }
                Some(Ok(Message::Ping(payload))) => NativeInbound::Ping(payload.to_vec()),
                Some(Ok(Message::Close(_))) | None => NativeInbound::Closed("asr-stream-timeout"),
                Some(Ok(Message::Binary(_))) => NativeInbound::Closed("asr-stream-protocol"),
                Some(Ok(Message::Pong(_))) | Some(Ok(Message::Frame(_))) => continue,
                Some(Err(_)) => NativeInbound::Closed("asr-stream-timeout"),
            };
            let closed = matches!(inbound, NativeInbound::Closed(_));
            if event_tx.send(inbound).await.is_err() || closed {
                break;
            }
        }
    });
    Ok(NativeConnection {
        sink: Box::new(WebSocketSink { writer }),
        events: event_rx,
        reader: reader_task,
    })
}

async fn wait_ready(
    socket: &mut Socket,
    session_id: &str,
    utterance_id: &str,
) -> Result<(), String> {
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    match harness_stream::decode_provider_event(text.as_bytes())? {
                        ProviderEvent::Ready {
                            session_id: actual_session,
                            utterance_id: actual_utterance,
                        } if actual_session == session_id && actual_utterance == utterance_id => {
                            return Ok(())
                        }
                        _ => return Err("asr-stream-protocol".to_string()),
                    }
                }
                Some(Ok(Message::Ping(payload))) => {
                    send_socket_with_timeout(socket, Message::Pong(payload)).await?;
                }
                Some(Ok(Message::Pong(_))) => {}
                _ => return Err("asr-stream-protocol".to_string()),
            }
        }
    })
    .await
    .map_err(|_| "asr-stream-timeout".to_string())?
}

async fn send_socket_with_timeout(socket: &mut Socket, message: Message) -> Result<(), String> {
    tokio::time::timeout(SEND_TIMEOUT, socket.send(message))
        .await
        .map_err(|_| "asr-stream-timeout".to_string())?
        .map_err(|_| "asr-stream-timeout".to_string())
}

async fn send_with_timeout(writer: &mut SocketWriter, message: Message) -> Result<(), String> {
    tokio::time::timeout(SEND_TIMEOUT, writer.send(message))
        .await
        .map_err(|_| "asr-stream-timeout".to_string())?
        .map_err(|_| "asr-stream-timeout".to_string())
}

#[cfg(test)]
pub(crate) fn fake_connection(
    sink: Box<dyn NativeAsrSink>,
) -> (NativeConnection, mpsc::Sender<NativeInbound>) {
    let (event_tx, event_rx) = mpsc::channel(EVENT_CAPACITY);
    let reader = tokio::spawn(std::future::pending());
    (
        NativeConnection {
            sink,
            events: event_rx,
            reader,
        },
        event_tx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{
            handshake::server::{Request, Response},
            http::HeaderValue,
        },
    };

    #[test]
    fn native_packet_size_matches_the_public_wire_contract() {
        assert_eq!(harness_stream::PACKET_BYTES, 3_200);
    }

    #[tokio::test]
    async fn mock_websocket_round_trip_meets_the_native_short_run_contract() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket =
                accept_hdr_async(stream, |request: &Request, mut response: Response| {
                    assert_eq!(
                        request
                            .headers()
                            .get("Sec-WebSocket-Protocol")
                            .and_then(|value| value.to_str().ok()),
                        Some(STREAM_PROTOCOL)
                    );
                    response.headers_mut().insert(
                        "Sec-WebSocket-Protocol",
                        HeaderValue::from_static(STREAM_PROTOCOL),
                    );
                    Ok(response)
                })
                .await
                .unwrap();

            let start = socket.next().await.unwrap().unwrap();
            let Message::Text(start) = start else {
                panic!("expected start event");
            };
            let start: serde_json::Value = serde_json::from_str(&start).unwrap();
            assert_eq!(start["contractVersion"], STREAM_PROTOCOL);
            socket
                .send(Message::Text(
                    r#"{"type":"ready","sessionId":"session_test","utteranceId":"utterance_test"}"#
                        .into(),
                ))
                .await
                .unwrap();

            let audio = socket.next().await.unwrap().unwrap();
            assert!(matches!(audio, Message::Binary(bytes) if bytes.len() == 3_200));
            socket
                .send(Message::Text(
                    r#"{"type":"partial","sessionId":"session_test","utteranceId":"utterance_test","revision":1,"startSample":0,"endSample":1600,"stableText":"hello","unstableText":" world","language":"en"}"#
                        .into(),
                ))
                .await
                .unwrap();

            let commit = socket.next().await.unwrap().unwrap();
            let Message::Text(commit) = commit else {
                panic!("expected commit event");
            };
            let commit: serde_json::Value = serde_json::from_str(&commit).unwrap();
            assert_eq!(commit["utteranceId"], "utterance_test");
            socket
                .send(Message::Text(
                    r#"{"type":"final","sessionId":"session_test","utteranceId":"utterance_test","revision":2,"startSample":0,"endSample":1600,"text":"hello world","language":"en"}"#
                        .into(),
                ))
                .await
                .unwrap();

            let stop = socket.next().await.unwrap().unwrap();
            let Message::Text(stop) = stop else {
                panic!("expected stop event");
            };
            let stop: serde_json::Value = serde_json::from_str(&stop).unwrap();
            assert_eq!(stop["sessionId"], "session_test");
        });

        let descriptor = AsrStreamingDescriptor {
            protocol: STREAM_PROTOCOL.to_string(),
            url: format!("ws://{address}/asr-stream"),
            sample_rate: 16_000,
            encoding: "pcm_s16le".to_string(),
            packet_milliseconds: 100,
        };
        let mut connection = open(
            &descriptor,
            "session_test",
            "utterance_test",
            "mock-model",
            "auto",
        )
        .await
        .unwrap();

        let speech_started = tokio::time::Instant::now();
        connection
            .sink
            .audio(Zeroizing::new(vec![7; 3_200]))
            .await
            .unwrap();
        let partial = tokio::time::timeout(Duration::from_secs(1), connection.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(speech_started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            partial,
            NativeInbound::Provider(ProviderEvent::Partial { revision: 1, .. })
        ));

        let silence_started = tokio::time::Instant::now();
        connection
            .sink
            .commit("utterance_test", "utterance_next", 1_600)
            .await
            .unwrap();
        let final_event = tokio::time::timeout(Duration::from_secs(1), connection.events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(silence_started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            final_event,
            NativeInbound::Provider(ProviderEvent::Final { revision: 2, .. })
        ));
        connection.sink.stop("session_test").await.unwrap();
        server.await.unwrap();
    }
}
