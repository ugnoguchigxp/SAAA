use async_trait::async_trait;
use serde_json::Value;

use super::{BackendOutcome, BackendRequest, TechnicalStatus, ToolBackend};
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
        if value.get("ok").and_then(Value::as_bool) == Some(true) {
            return BackendOutcome::succeeded(value);
        }
        let code = stable_reason(
            value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("webview-failed"),
        );
        BackendOutcome {
            status: TechnicalStatus::Failed,
            result: Some(value),
            error_code: Some(code),
        }
    }
}

fn stable_reason(reason: &str) -> &'static str {
    match reason {
        "webview-not-operable" => "webview-not-operable",
        "webview-not-scrollable" => "webview-not-scrollable",
        "webview-operation-unknown" => "webview-operation-unknown",
        "webview-state-changed" => "webview-state-changed",
        "webview-timeout" => "webview-timeout",
        "webview-unavailable" => "webview-unavailable",
        "tab-index-missing" => "tab-index-missing",
        "tab-index-out-of-range" => "tab-index-out-of-range",
        "no-website-tabs" => "no-website-tabs",
        _ => "webview-failed",
    }
}
