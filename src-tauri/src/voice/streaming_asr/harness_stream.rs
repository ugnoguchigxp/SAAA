use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
    MaybeTlsStream, WebSocketStream,
};

use crate::providers::service_harness::AsrStreamingDescriptor;

pub(crate) const STREAM_PROTOCOL: &str = "saaa.asr-stream.v1";
pub(crate) const PACKET_BYTES: usize = 3_200;
pub(crate) const MAX_TEXT_BYTES: usize = 65_536;

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
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
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
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
    let mut request = descriptor
        .url
        .clone()
        .into_client_request()
        .map_err(|_| "asr-stream-protocol".to_string())?;
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(STREAM_PROTOCOL),
    );
    let (socket, response) =
        tokio::time::timeout(std::time::Duration::from_secs(5), connect_async(request))
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
    }
    #[test]
    fn rejects_unknown_provider_event() {
        assert!(decode_provider_event(br#"{"type":"wat"}"#).is_err());
    }
}
