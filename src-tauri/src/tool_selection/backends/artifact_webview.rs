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
        dispatch::invoke(request).await
    }
}

#[path = "artifact_webview/dispatch.rs"]
mod dispatch;
