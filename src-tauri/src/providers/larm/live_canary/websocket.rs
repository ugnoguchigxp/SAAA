use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{CanaryError, LarmProvider};

#[derive(Clone, Default)]
struct CanaryEvents {
    content: Arc<Mutex<String>>,
    cancel_on_delta: Option<Arc<crate::RunCancellation>>,
}

impl crate::runtime::event_hub::RuntimeEventSender for CanaryEvents {
    fn send(&self, event: crate::ipc_contract::RuntimeEvent) -> tauri::Result<()> {
        if let crate::ipc_contract::RuntimeEvent::Delta { text, .. } = event {
            self.content
                .lock()
                .expect("canary content lock")
                .push_str(&text);
            if let Some(cancellation) = &self.cancel_on_delta {
                cancellation.cancel();
            }
        }
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn crate::runtime::event_hub::RuntimeEventSender> {
        Box::new(self.clone())
    }
}

pub(super) async fn websocket_turn(
    provider: &LarmProvider<'_>,
    lease: &super::super::ReadyLease,
    prompt: &str,
    cancel_on_delta: bool,
) -> Result<crate::providers::llm_websocket::client::WebSocketRunResult, CanaryError> {
    let cancellation = Arc::new(crate::RunCancellation::default());
    let events = CanaryEvents {
        content: Arc::new(Mutex::new(String::new())),
        cancel_on_delta: cancel_on_delta.then(|| cancellation.clone()),
    };
    let run_id = format!("run_canary_{}", uuid::Uuid::new_v4().simple());
    let input = crate::StartTurnInput {
        run_id,
        conversation_id: "conversation_larm_canary".to_string(),
        content: prompt.to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let messages = [serde_json::json!({ "role": "user", "content": prompt })];
    let stream_url = provider
        .websocket_stream_url()
        .map_err(|_| CanaryError::Contract)?;
    let authorization = provider
        .websocket_authorization()
        .map_err(|_| CanaryError::Authentication)?;
    crate::providers::llm_websocket::client::run(
        crate::providers::llm_websocket::client::WebSocketRunContext {
            stream_url: stream_url.as_str(),
            authorization: Some(authorization),
            allocation_id: Some(lease.allocation_id.as_str()),
            model: "local",
            messages: &messages,
            tools: &[],
            reasoning_effort: crate::providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            max_output_tokens: crate::providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            tool_timeout: Duration::from_secs(60),
            timeout: Duration::from_secs(120),
            input: &input,
            on_event: &events,
            cancellation,
            output_persistence: None,
        },
    )
    .await
    .map_err(|_| CanaryError::Gateway)
}
