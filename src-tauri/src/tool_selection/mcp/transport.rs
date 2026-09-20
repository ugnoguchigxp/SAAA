//! Minimal JSON-RPC 2.0 client for the MCP Streamable HTTP transport.
//!
//! Both POST response shapes are handled: a plain `application/json` body and a
//! `text/event-stream` body. The SSE decoder is a small state machine so chunk boundaries,
//! multiple `data:` lines, CRLF, comments and split UTF-8 sequences are all covered by unit tests.
//! Redirects are never followed, proxy environment variables are never consulted implicitly, and
//! TLS verification is never disabled.

use futures_util::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::mpsc;

use super::{
    MCP_CALL_RESPONSE_MAX_BYTES, MCP_CONNECT_TIMEOUT, MCP_PROTOCOL_VERSION, MCP_SSE_EVENT_MAX_BYTES,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The deadline elapsed (connect, initialize or call).
    Timeout,
    /// The TCP/TLS connection could not be established or was lost.
    Connect,
    /// A non-success HTTP status other than the handled 202/404/405 cases.
    Http(u16),
    /// The server rejected the session (HTTP 404); the caller re-initializes for the next work.
    SessionExpired,
    /// A response could not be parsed as the expected shape.
    Protocol(&'static str),
    /// A response, SSE event or accumulated body exceeded a documented limit.
    BodyTooLarge,
    /// A JSON-RPC error object addressed to this request id.
    Rpc { code: i64 },
    /// The transport is closed (shutdown).
    Closed,
}

impl TransportError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Timeout => "remote-timeout",
            Self::Connect => "remote-connect",
            Self::Http(_) => "remote-http",
            Self::SessionExpired => "session-expired",
            Self::Protocol(_) => "remote-protocol",
            Self::BodyTooLarge => "remote-too-large",
            Self::Rpc { .. } => "remote-rpc-error",
            Self::Closed => "remote-closed",
        }
    }

    /// Only transport failures observed before the request was written are safe to treat as "not
    /// sent". A timeout or a dropped response is an indeterminate remote outcome.
    pub fn indeterminate(&self) -> bool {
        matches!(self, Self::Timeout | Self::Connect)
    }
}

/// One parsed Server-Sent Event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental SSE decoder. `push` may be called with any byte boundary; an incomplete UTF-8
/// sequence at the end of a chunk is retained until the continuation arrives.
#[derive(Default)]
pub struct SseDecoder {
    pending: Vec<u8>,
    current: SseEvent,
    saw_data: bool,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();
        // Split on '\n'. Any trailing partial line stays in the buffer.
        let mut start = 0;
        while let Some(relative) = self.pending[start..].iter().position(|byte| *byte == b'\n') {
            let end = start + relative;
            let mut line = self.pending[start..end].to_vec();
            start = end + 1;
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let text = match String::from_utf8(line) {
                Ok(text) => text,
                Err(_) => {
                    // A malformed sequence cannot be repaired; drop the line.
                    self.current = SseEvent::default();
                    self.saw_data = false;
                    continue;
                }
            };
            if text.is_empty() {
                if self.saw_data {
                    events.push(std::mem::take(&mut self.current));
                    self.saw_data = false;
                }
                continue;
            }
            if text.starts_with(':') {
                continue;
            }
            let (field, value) = match text.split_once(':') {
                Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
                None => (text.as_str(), ""),
            };
            match field {
                "data" => {
                    if self.saw_data {
                        self.current.data.push('\n');
                    }
                    self.current.data.push_str(value);
                    self.saw_data = true;
                }
                "event" => {
                    self.current.event = Some(value.to_string());
                }
                _ => {}
            }
        }
        self.pending.drain(..start);
        events
    }

    /// Flushes a final event with no trailing blank line.
    pub fn finish(&mut self) -> Option<SseEvent> {
        if self.saw_data {
            self.saw_data = false;
            return Some(std::mem::take(&mut self.current));
        }
        None
    }
}

/// Reads `data:` JSON-RPC messages from a byte stream, enforcing the event size bound.
pub struct SseReader {
    decoder: SseDecoder,
    total_bytes: usize,
}

impl SseReader {
    pub fn new() -> Self {
        Self {
            decoder: SseDecoder::new(),
            total_bytes: 0,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, TransportError> {
        self.total_bytes += chunk.len();
        if self.total_bytes > MCP_CALL_RESPONSE_MAX_BYTES {
            return Err(TransportError::BodyTooLarge);
        }
        let events = self.decoder.push(chunk);
        let mut messages = Vec::new();
        for event in events {
            if event.data.len() > MCP_SSE_EVENT_MAX_BYTES {
                return Err(TransportError::BodyTooLarge);
            }
            match serde_json::from_str::<Value>(&event.data) {
                Ok(value) => messages.push(value),
                Err(_) => return Err(TransportError::Protocol("sse-data")),
            }
        }
        Ok(messages)
    }

    pub fn finish(&mut self) -> Result<Vec<Value>, TransportError> {
        let mut messages = Vec::new();
        if let Some(event) = self.decoder.finish() {
            match serde_json::from_str::<Value>(&event.data) {
                Ok(value) => messages.push(value),
                Err(_) => return Err(TransportError::Protocol("sse-data")),
            }
        }
        Ok(messages)
    }
}

impl Default for SseReader {
    fn default() -> Self {
        Self::new()
    }
}

/// A client for exactly one MCP Streamable HTTP endpoint. Session state lives here so the
/// session state machine above does not need to know about HTTP headers.
pub struct HttpTransport {
    client: reqwest::Client,
    url: String,
    token: Option<String>,
    session_id: Mutex<Option<String>>,
    protocol_version: String,
    closed: Mutex<bool>,
}

pub type EventReceiver = mpsc::UnboundedReceiver<Value>;

impl HttpTransport {
    pub fn new(url: &str, token: Option<String>) -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(MCP_CONNECT_TIMEOUT)
            .build()
            .map_err(|_| TransportError::Connect)?;
        Ok(Self {
            client,
            url: url.to_string(),
            token,
            session_id: Mutex::new(None),
            protocol_version: MCP_PROTOCOL_VERSION.to_string(),
            closed: Mutex::new(false),
        })
    }

    pub fn set_session_id(&self, session_id: Option<String>) {
        if let Ok(mut current) = self.session_id.lock() {
            *current = session_id;
        }
    }

    pub fn session_id(&self) -> Option<String> {
        self.session_id.lock().ok().and_then(|value| value.clone())
    }

    pub fn close(&self) {
        if let Ok(mut closed) = self.closed.lock() {
            *closed = true;
        }
    }

    fn is_closed(&self) -> bool {
        self.closed.lock().map(|closed| *closed).unwrap_or(true)
    }

    fn build_request(
        &self,
        builder: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, TransportError> {
        if self.is_closed() {
            return Err(TransportError::Closed);
        }
        let mut builder = builder
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .header("MCP-Protocol-Version", &self.protocol_version);
        if let Some(session) = self.session_id() {
            builder = builder.header("Mcp-Session-Id", session);
        }
        if let Some(token) = &self.token {
            builder = builder.bearer_auth(token);
        }
        Ok(builder)
    }

    /// Sends one JSON-RPC request. `id` must be a non-empty string and the response id must match
    /// exactly (value and type).
    pub async fn request(
        &self,
        id: &str,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, TransportError> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let builder = self.build_request(self.client.post(&self.url))?;
        let response = tokio::time::timeout(deadline, builder.json(&payload).send())
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|_| TransportError::Connect)?;
        self.handle_response(id, response, deadline).await
    }

    /// Sends a notification (no id). A `202 Accepted` with an empty body is the normal reply.
    pub async fn notify(
        &self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<(), TransportError> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let builder = self.build_request(self.client.post(&self.url))?;
        let response = tokio::time::timeout(deadline, builder.json(&payload).send())
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|_| TransportError::Connect)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(TransportError::SessionExpired);
        }
        Ok(())
    }

    /// Sends a JSON-RPC response to a server-initiated request. Used only to refuse unsupported
    /// server requests with `-32601`.
    pub async fn reply_error(
        &self,
        id: &Value,
        code: i64,
        message: &str,
        deadline: Duration,
    ) -> Result<(), TransportError> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message },
        });
        let builder = self.build_request(self.client.post(&self.url))?;
        tokio::time::timeout(deadline, builder.json(&payload).send())
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|_| TransportError::Connect)?;
        Ok(())
    }

    /// Deletes the server-side session. A 405 is treated as "DELETE not offered" and is not an
    /// error.
    pub async fn delete_session(&self) -> Result<(), TransportError> {
        let Some(_) = self.session_id() else {
            return Ok(());
        };
        let builder = self.build_request(self.client.delete(&self.url))?;
        let response = tokio::time::timeout(MCP_CONNECT_TIMEOUT, builder.send())
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|_| TransportError::Connect)?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        self.set_session_id(None);
        Ok(())
    }

    /// Opens the optional GET SSE stream. A 405 means the server does not offer it, which is a
    /// normal answer and returns `None`.
    pub async fn open_event_stream(&self) -> Option<EventReceiver> {
        let builder = self
            .build_request(self.client.get(&self.url))
            .ok()?
            .header(ACCEPT, "text/event-stream");
        let response = builder.send().await.ok()?;
        if response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
            return None;
        }
        if !response.status().is_success() {
            return None;
        }
        let (sender, receiver) = mpsc::unbounded_channel();
        let url = self.url.clone();
        let token = self.token.clone();
        let session = self.session_id();
        tokio::spawn(async move {
            let mut reader = SseReader::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let Ok(chunk) = chunk else { break };
                let Ok(messages) = reader.push(&chunk) else {
                    break;
                };
                for message in messages {
                    if message.get("method").is_some() && message.get("id").is_some() {
                        // An unsupported server request: refuse it and never execute it.
                        if let Ok(transport) = HttpTransport::new(&url, token.clone()) {
                            transport.set_session_id(session.clone());
                            let id = message.get("id").cloned().unwrap_or(Value::Null);
                            let _ = transport
                                .reply_error(
                                    &id,
                                    -32601,
                                    "Method not supported by this client.",
                                    MCP_CONNECT_TIMEOUT,
                                )
                                .await;
                            transport.close();
                        }
                    } else if let Err(_) = sender.send(message) {
                        break;
                    }
                }
            }
        });
        Some(receiver)
    }

    async fn handle_response(
        &self,
        id: &str,
        response: reqwest::Response,
        deadline: Duration,
    ) -> Result<Value, TransportError> {
        let status = response.status();
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            self.set_session_id(Some(session.to_string()));
        }
        match status {
            reqwest::StatusCode::NOT_FOUND => return Err(TransportError::SessionExpired),
            reqwest::StatusCode::ACCEPTED => return Ok(Value::Null),
            _ if !status.is_success() => return Err(TransportError::Http(status.as_u16())),
            _ => {}
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if content_type.contains("text/event-stream") {
            return self.read_sse(id, response, deadline).await;
        }
        let body = tokio::time::timeout(deadline, response.bytes())
            .await
            .map_err(|_| TransportError::Timeout)?
            .map_err(|_| TransportError::Connect)?;
        if body.len() > MCP_CALL_RESPONSE_MAX_BYTES {
            return Err(TransportError::BodyTooLarge);
        }
        let message: Value =
            serde_json::from_slice(&body).map_err(|_| TransportError::Protocol("json-body"))?;
        extract_result(id, &message)
    }

    async fn read_sse(
        &self,
        id: &str,
        response: reqwest::Response,
        deadline: Duration,
    ) -> Result<Value, TransportError> {
        let mut reader = SseReader::new();
        let mut stream = response.bytes_stream();
        let mut notifications = 0_usize;
        let mut notification_bytes = 0_usize;
        let outcome = tokio::time::timeout(deadline, async {
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| TransportError::Connect)?;
                for message in reader.push(&chunk)? {
                    if matches_id(id, &message) {
                        return extract_result(id, &message);
                    }
                    if message.get("id").is_none() {
                        notifications += 1;
                        notification_bytes += message.to_string().len();
                        if notifications > 1024 || notification_bytes > MCP_SSE_EVENT_MAX_BYTES {
                            return Err(TransportError::BodyTooLarge);
                        }
                    }
                }
            }
            for message in reader.finish()? {
                if matches_id(id, &message) {
                    return extract_result(id, &message);
                }
            }
            Err(TransportError::Protocol("sse-eof"))
        })
        .await
        .map_err(|_| TransportError::Timeout)?;
        outcome
    }
}

fn matches_id(id: &str, message: &Value) -> bool {
    message.get("id").and_then(Value::as_str) == Some(id)
}

fn extract_result(id: &str, message: &Value) -> Result<Value, TransportError> {
    match message.get("id").and_then(Value::as_str) {
        Some(response_id) if response_id == id => {
            if let Some(error) = message.get("error") {
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                return Err(TransportError::Rpc { code });
            }
            message
                .get("result")
                .cloned()
                .ok_or(TransportError::Protocol("missing-result"))
        }
        Some(_) => Err(TransportError::Protocol("id-mismatch")),
        None => Err(TransportError::Protocol("missing-id")),
    }
}

/// Collects the byte stream of a JSON response without the SSE machinery. Kept for the GET path.
pub async fn drain_json(response: reqwest::Response) -> Result<Value, TransportError> {
    let bytes = response.bytes().await.map_err(|_| TransportError::Connect)?;
    if bytes.len() > MCP_CALL_RESPONSE_MAX_BYTES {
        return Err(TransportError::BodyTooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| TransportError::Protocol("json-body"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_decoder_handles_chunk_boundaries_and_multiple_data_lines() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"event: message\ndata: {\"a\":1,").is_empty());
        let events = decoder.push(b"\ndata: \"b\"}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message"));
        assert_eq!(events[0].data, "{\"a\":1,\n\"b\"}");
    }

    #[test]
    fn sse_decoder_handles_crlf_and_comments() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b": keep-alive\r\ndata: 1\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "1");
    }

    #[test]
    fn sse_decoder_preserves_split_utf8() {
        let mut decoder = SseDecoder::new();
        let text = "こんにちは";
        let mut bytes = b"data: ".to_vec();
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\n\n");
        let (first, second) = bytes.split_at(8);
        assert!(decoder.push(first).is_empty());
        let events = decoder.push(second);
        assert_eq!(events[0].data, text);
    }

    #[test]
    fn sse_reader_rejects_oversized_bodies() {
        let mut reader = SseReader::new();
        let chunk = vec![b'x'; MCP_CALL_RESPONSE_MAX_BYTES + 1];
        assert_eq!(reader.push(&chunk), Err(TransportError::BodyTooLarge));
    }

    #[test]
    fn rpc_error_is_extracted_with_the_code() {
        let message = json!({"jsonrpc":"2.0","id":"c1","error":{"code":-32601,"message":"no"}});
        assert_eq!(
            extract_result("c1", &message),
            Err(TransportError::Rpc { code: -32601 })
        );
    }

    #[test]
    fn id_type_mismatch_is_a_protocol_error() {
        let message = json!({"jsonrpc":"2.0","id":7,"result":{}});
        assert_eq!(
            extract_result("7", &message),
            Err(TransportError::Protocol("missing-id"))
        );
    }
}
