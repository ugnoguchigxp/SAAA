use async_trait::async_trait;
use serde_json::Value;

use super::{BackendOutcome, BackendRequest, ToolBackend};
use crate::RunCancellation;

pub struct ArtifactWebviewBackend;

#[async_trait]
impl ToolBackend for ArtifactWebviewBackend {
    async fn invoke(
        &self,
        request: BackendRequest,
        _cancellation: &RunCancellation,
    ) -> BackendOutcome {
        let operation = request
            .arguments
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let index = request
            .arguments
            .get("index")
            .and_then(Value::as_u64)
            .map(|value| value as usize);
        let conversation_id = request
            .actor
            .as_ref()
            .map(|actor| actor.conversation_id.clone())
            .unwrap_or_default();
        let value = tokio::task::spawn_blocking(move || {
            crate::artifact_preview::webview_ops::execute(&operation, index, &conversation_id)
        })
        .await
        .unwrap_or_else(|_| serde_json::json!({"ok": false, "reason": "webview-unavailable"}));
        BackendOutcome::succeeded(value)
    }
}
