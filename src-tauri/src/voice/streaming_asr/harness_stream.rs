use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        client::IntoClientRequest, http::HeaderValue, protocol::WebSocketConfig, Message,
    },
    MaybeTlsStream, WebSocketStream,
};

use crate::providers::service_harness::AsrStreamingDescriptor;

pub(crate) const STREAM_PROTOCOL: &str = "saaa.asr-stream.v1";
pub(crate) const PACKET_BYTES: usize = 3_200;
pub(crate) const MAX_TEXT_BYTES: usize = 65_536;

#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ClientEvent<'a> {
    Start {
        contract_version: &'static str,
        session_id: &'a str,
        utterance_id: &'a str,
        model: &'a str,
        sample_rate: u32,
        encoding: &'static str,
        packet_milliseconds: u64,
        language: &'a str,
    },
    Commit {
        utterance_id: &'a str,
        next_utterance_id: &'a str,
        end_sample: u64,
    },
    Stop {
        session_id: &'a str,
    },
}
#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum ProviderEvent {
    Ready {
        session_id: String,
        utterance_id: String,
    },
    Partial {
        session_id: String,
        utterance_id: String,
        revision: u64,
        start_sample: u64,
        end_sample: u64,
        stable_text: String,
        unstable_text: String,
        language: Option<String>,
    },
    Final {
        session_id: String,
        utterance_id: String,
        revision: u64,
        start_sample: u64,
        end_sample: u64,
        text: String,
        language: Option<String>,
    },
    NoSpeech {
        session_id: String,
        utterance_id: String,
        revision: u64,
        start_sample: u64,
        end_sample: u64,
    },
    Error {
        session_id: String,
        utterance_id: Option<String>,
        code: String,
        message: String,
        recoverable: bool,
    },
    Stopped {
        session_id: String,
    },
}
pub(crate) fn validate_descriptor(descriptor: &AsrStreamingDescriptor) -> Result<(), String> {
    if descriptor.protocol != STREAM_PROTOCOL
        || descriptor.sample_rate != 16_000
        || descriptor.encoding != "pcm_s16le"
        || descriptor.packet_milliseconds != 100
    {
        return Err("asr-stream-protocol".to_string());
    }
    Ok(())
}
pub(crate) fn encode_client_event(event: &ClientEvent<'_>) -> Result<String, String> {
    serde_json::to_string(event).map_err(|_| "asr-stream-protocol".to_string())
}
pub(crate) fn decode_provider_event(text: &[u8]) -> Result<ProviderEvent, String> {
    if text.len() > MAX_TEXT_BYTES {
        return Err("asr-stream-protocol".to_string());
    }
    let event: ProviderEvent =
        serde_json::from_slice(text).map_err(|_| "asr-stream-protocol".to_string())?;
    match &event {
        ProviderEvent::Partial {
            stable_text,
            unstable_text,
            ..
        } if stable_text.chars().count() + unstable_text.chars().count() > 16_000 => {
            Err("asr-stream-protocol".to_string())
        }
        ProviderEvent::Final { text, .. } if text.chars().count() > 16_000 => {
            Err("asr-stream-protocol".to_string())
        }
        _ => Ok(event),
    }
}
pub(crate) async fn connect(
    descriptor: &AsrStreamingDescriptor,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, String> {
    validate_descriptor(descriptor)?;
    let request = stream_request(descriptor)?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_TEXT_BYTES))
        .max_frame_size(Some(MAX_TEXT_BYTES));
    let (socket, response) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        connect_async_with_config(request, Some(config), false),
    )
    .await
    .map_err(|_| "asr-stream-timeout".to_string())?
    .map_err(|_| "asr-stream-protocol".to_string())?;
    if response
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|value| value.to_str().ok())
        != Some(STREAM_PROTOCOL)
    {
        return Err("asr-stream-protocol".to_string());
    }
    Ok(socket)
}

fn stream_request(
    descriptor: &AsrStreamingDescriptor,
) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
    validate_descriptor(descriptor)?;
    let mut request = descriptor
        .url
        .clone()
        .into_client_request()
        .map_err(|_| "asr-stream-protocol".to_string())?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(STREAM_PROTOCOL),
    );
    Ok(request)
}
pub(crate) fn audio_message(bytes: Vec<u8>) -> Result<Message, String> {
    if bytes.len() == PACKET_BYTES {
        Ok(Message::Binary(bytes.into()))
    } else {
        Err("asr-packet-format".to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serializes_start_with_fixed_contract() {
        let event = encode_client_event(&ClientEvent::Start {
            contract_version: STREAM_PROTOCOL,
            session_id: "s",
            utterance_id: "u",
            model: "m",
            sample_rate: 16_000,
            encoding: "pcm_s16le",
            packet_milliseconds: 100,
            language: "auto",
        })
        .unwrap();
        assert!(event.contains(STREAM_PROTOCOL));
        assert!(event.contains("\"contractVersion\""));
    }
    #[test]
    fn websocket_request_requires_the_exact_subprotocol() {
        let descriptor = AsrStreamingDescriptor {
            protocol: STREAM_PROTOCOL.to_string(),
            url: "ws://127.0.0.1:9810/asr".to_string(),
            sample_rate: 16_000,
            encoding: "pcm_s16le".to_string(),
            packet_milliseconds: 100,
        };
        let request = stream_request(&descriptor).unwrap();
        assert_eq!(
            request.headers().get("Sec-WebSocket-Protocol").unwrap(),
            STREAM_PROTOCOL
        );
    }
    #[test]
    fn rejects_unknown_provider_event() {
        assert!(decode_provider_event(br#"{"type":"wat"}"#).is_err());
    }
    #[test]
    fn rejects_unknown_fields_and_oversized_text_frames() {
        assert!(decode_provider_event(
            br#"{"type":"ready","sessionId":"s","utteranceId":"u","extra":true}"#
        )
        .is_err());
        assert!(decode_provider_event(&vec![b'x'; MAX_TEXT_BYTES + 1]).is_err());
    }
    #[test]
    fn commit_and_stop_are_typed_and_audio_is_exactly_one_packet() {
        let commit = encode_client_event(&ClientEvent::Commit {
            utterance_id: "u1",
            next_utterance_id: "u2",
            end_sample: 1_600,
        })
        .unwrap();
        assert!(commit.contains("\"endSample\":1600"));
        assert!(audio_message(vec![0; PACKET_BYTES]).is_ok());
        assert!(audio_message(vec![0; PACKET_BYTES - 1]).is_err());
    }
    #[test]
    fn decodes_partial_final_and_no_speech_with_camel_case_samples() {
        let partial = decode_provider_event(br#"{"type":"partial","sessionId":"s","utteranceId":"u","revision":1,"startSample":0,"endSample":1600,"stableText":"a","unstableText":"b","language":"en"}"#).unwrap();
        assert!(matches!(
            partial,
            ProviderEvent::Partial {
                end_sample: 1_600,
                ..
            }
        ));
        let final_event = decode_provider_event(br#"{"type":"final","sessionId":"s","utteranceId":"u","revision":2,"startSample":0,"endSample":1600,"text":"ab","language":"en"}"#).unwrap();
        assert!(matches!(
            final_event,
            ProviderEvent::Final { revision: 2, .. }
        ));
        let no_speech = decode_provider_event(br#"{"type":"noSpeech","sessionId":"s","utteranceId":"u","revision":1,"startSample":0,"endSample":1600}"#).unwrap();
        assert!(matches!(no_speech, ProviderEvent::NoSpeech { .. }));
    }
}
