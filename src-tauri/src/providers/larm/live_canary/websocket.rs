use std::sync::{Arc, Mutex};

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
    allocation: &super::super::contracts::ReadyAllocation,
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
    let history = [crate::ipc_contract::ConversationMessage {
        id: "message_larm_canary".to_string(),
        conversation_id: input.conversation_id.clone(),
        role: "user".to_string(),
        content: prompt.to_string(),
        created_at: "canary".to_string(),
    }];
    let stream_url = provider
        .websocket_stream_url()
        .map_err(|_| CanaryError::Contract)?;
    let authorization = provider
        .websocket_authorization()
        .map_err(|_| CanaryError::Authentication)?;
    crate::providers::stream::run_model_websocket(
        stream_url.as_str(),
        Some(authorization),
        Some(allocation.allocation_id.as_str()),
        "local",
        &history,
        120_000,
        crate::providers::stream::ModelStreamContext {
            reasoning_effort: crate::providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            max_output_tokens: crate::providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &events,
            cancellation,
            output_persistence: None,
        },
    )
    .await
    .map_err(|_| CanaryError::Gateway)
}
