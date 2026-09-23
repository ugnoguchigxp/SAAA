//! External MCP backend adapter. It validates the DB-derived binding, re-checks source
//! eligibility through the manager gate, and forwards one `tools/call`. It never reads a URL or a
//! kind from the model's arguments.

#![allow(private_interfaces)]

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

use super::{BackendOutcome, BackendRequest, TechnicalStatus, ToolBackend};
use crate::tool_selection::mcp::manager::McpManager;
use crate::tool_selection::mcp::session::CallError;
use crate::RunCancellation;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpBinding {
    pub kind: String,
    pub source_id: String,
    pub tool_name: String,
    pub endpoint_hash: String,
}

impl McpBinding {
    pub fn parse(binding: &Value) -> Option<Self> {
        let parsed: Self = serde_json::from_value(binding.clone()).ok()?;
        if parsed.kind != "mcp_http"
            || parsed.source_id.is_empty()
            || parsed.tool_name.is_empty()
            || parsed.endpoint_hash.len() != 64
            || !parsed.endpoint_hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            return None;
        }
        Some(parsed)
    }
}

pub struct McpBackend {
    pub(super) manager: Arc<McpManager>,
}

impl McpBackend {
    pub fn new(manager: Arc<McpManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ToolBackend for McpBackend {
    async fn invoke(
        &self,
        request: BackendRequest,
        cancellation: &RunCancellation,
    ) -> BackendOutcome {
        let Some(binding) = McpBinding::parse(&request.binding) else {
            // A malformed or unknown binding is refused before any HTTP write.
            return BackendOutcome::failed("integrity");
        };
        if binding.tool_name != request.backend_key {
            return BackendOutcome::failed("stale-reference");
        }
        let timeout = request.timeout.min(Duration::from_secs(30));
        match self
            .manager
            .invoke(
                &binding.source_id,
                &binding.endpoint_hash,
                &binding.tool_name,
                request.arguments.clone(),
                timeout,
                cancellation,
            )
            .await
        {
            Ok(result) => outcome_from_result(&result),
            Err(CallError::CancelledBeforeSend) => BackendOutcome::cancelled(),
            Err(CallError::RpcError) => BackendOutcome {
                status: TechnicalStatus::Failed,
                result: None,
                error_code: Some("remote-rpc-error"),
            },
            Err(CallError::Busy) => BackendOutcome::failed("capacity"),
            Err(CallError::Closed) => BackendOutcome::failed("unavailable"),
            Err(CallError::Unavailable(code)) => BackendOutcome::failed(code),
            Err(CallError::SessionExpired) => BackendOutcome {
                status: TechnicalStatus::Unknown,
                result: None,
                error_code: Some("session-expired"),
            },
            Err(CallError::Protocol(code)) => BackendOutcome {
                status: TechnicalStatus::Unknown,
                result: None,
                error_code: Some(code),
            },
            Err(CallError::Unknown(code)) => BackendOutcome {
                status: TechnicalStatus::Unknown,
                result: None,
                error_code: Some(code),
            },
        }
    }
}

fn outcome_from_result(result: &Value) -> BackendOutcome {
    if result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return BackendOutcome {
            status: TechnicalStatus::Failed,
            result: Some(result.clone()),
            error_code: Some("remote-tool-error"),
        };
    }
    BackendOutcome::succeeded(result.clone())
}
