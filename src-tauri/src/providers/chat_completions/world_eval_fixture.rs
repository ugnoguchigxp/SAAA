//! Local wire fixture for the World acceptance suite. No credentials or external services.
use super::*;
use crate::runtime::event_hub::RuntimeEventSender;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Default)]
pub(super) struct Sink;
impl RuntimeEventSender for Sink {
    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }
    fn send(&self, _: RuntimeEvent) -> tauri::Result<()> {
        Ok(())
    }
}

pub(super) struct Wire {
    pub endpoint: String,
    pub bodies: Arc<Mutex<Vec<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Wire {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Wire {
    pub async fn start(after_first: Option<Box<dyn FnOnce() + Send>>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let captured = bodies.clone();
        let task = tokio::spawn(async move {
            let mut hook = after_first;
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                let (start, length) = loop {
                    let mut chunk = [0; 8192];
                    let count = stream.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..end]);
                        let length = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|s| s.parse::<usize>().ok())
                            })
                            .unwrap();
                        break (end + 4, length);
                    }
                };
                while bytes.len() < start + length {
                    let mut chunk = [0; 8192];
                    let count = stream.read(&mut chunk).await.unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&chunk[..count]);
                }
                let request: Value = serde_json::from_slice(&bytes[start..start + length]).unwrap();
                captured
                    .lock()
                    .unwrap()
                    .push(bytes[start..start + length].to_vec());
                let tool = hook.take();
                let message = if let Some(change) = tool {
                    change();
                    json!({"role":"assistant","content":null,"tool_calls":[{"id":"ui-world","type":"function","function":{"name":"present_ui","arguments":json!({"definition":"root=ModelStatus(\"larm.status\")","summary":"合成結果","mode":"live"}).to_string()}}]})
                } else {
                    json!({"role":"assistant","content":"fixture complete"})
                };
                let finish = if message.get("tool_calls").is_some() {
                    "tool_calls"
                } else {
                    "stop"
                };
                let body = if request["stream"] == true {
                    format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        json!({"model":"fixture","choices":[{"index":0,"delta":message,"finish_reason":finish}]})
                    )
                } else {
                    json!({"model":"fixture","choices":[{"index":0,"message":message,"finish_reason":finish}]}).to_string()
                };
                let media = if request["stream"] == true {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {media}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        Self {
            endpoint,
            bodies,
            task,
        }
    }
}

pub(super) fn state(writer: Arc<crate::persistence::SqliteWriter>) -> crate::AppState {
    let capabilities = Arc::new(
        crate::generated_capabilities::service::CapabilityService::build(
            writer.clone(),
            &std::path::PathBuf::new(),
            std::path::PathBuf::new(),
            None,
        ),
    );
    crate::test_state::app_state_with_capabilities(writer, capabilities)
}

pub(super) fn frame(body: &Value) -> Option<Value> {
    for message in body["messages"].as_array().unwrap() {
        if message["role"] != "assistant" {
            continue;
        }
        let Some(content) = message["content"].as_str() else {
            continue;
        };
        if let Some((_, rest)) =
            content.split_once(crate::runtime::context::world::render::WORLD_HEADER)
        {
            let (json, _) = rest
                .split_once(crate::runtime::context::world::render::WORLD_FOOTER)
                .unwrap();
            return Some(serde_json::from_str(json.trim()).unwrap());
        }
    }
    None
}
