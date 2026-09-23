//! Backend boundary. L-Lang and (later) external MCP adapters implement `ToolBackend`; tests use
//! the fixture backend so the deterministic fixtures never depend on a live host.

#![allow(private_interfaces)]

pub mod artifact_webview;
pub mod llang;
pub mod mcp;
pub mod router;

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use crate::generated_capabilities::contracts::CallActor;
use crate::RunCancellation;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TechnicalStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
    Interrupted,
}

impl TechnicalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Debug)]
pub struct BackendRequest {
    pub call_id: String,
    pub tool_id: String,
    pub revision_id: String,
    pub backend_key: String,
    pub binding: Value,
    pub arguments: Value,
    pub timeout: Duration,
    /// `conversation` or `mcp`; propagated to the generated-capability call row.
    pub origin: &'static str,
    /// Host-derived ownership, absent for an internal call with no recorded owner.
    pub actor: Option<CallActor>,
}

#[derive(Clone, Debug)]
pub struct BackendOutcome {
    pub status: TechnicalStatus,
    pub result: Option<Value>,
    pub error_code: Option<&'static str>,
}

impl BackendOutcome {
    pub fn succeeded(result: Value) -> Self {
        Self {
            status: TechnicalStatus::Succeeded,
            result: Some(result),
            error_code: None,
        }
    }

    pub fn failed(code: &'static str) -> Self {
        Self {
            status: TechnicalStatus::Failed,
            result: None,
            error_code: Some(code),
        }
    }

    pub fn cancelled() -> Self {
        Self {
            status: TechnicalStatus::Cancelled,
            result: None,
            error_code: Some("cancelled"),
        }
    }
}

#[async_trait]
pub trait ToolBackend: Send + Sync {
    async fn invoke(
        &self,
        request: BackendRequest,
        cancellation: &RunCancellation,
    ) -> BackendOutcome;
}

/// Deterministic backend double. Records every call and returns a configured outcome per tool.
pub struct FixtureBackend {
    outcomes: Mutex<HashMap<String, BackendOutcome>>,
    calls: Mutex<Vec<BackendRequest>>,
    default_result: Mutex<Value>,
}

impl Default for FixtureBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureBackend {
    pub fn new() -> Self {
        Self {
            outcomes: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
            default_result: Mutex::new(serde_json::json!({"ok": true, "value": true})),
        }
    }
    pub fn call_count(&self) -> usize {
        self.calls.lock().map(|calls| calls.len()).unwrap_or(0)
    }
}

#[async_trait]
impl ToolBackend for FixtureBackend {
    async fn invoke(
        &self,
        request: BackendRequest,
        cancellation: &RunCancellation,
    ) -> BackendOutcome {
        if cancellation.is_cancelled() {
            return BackendOutcome::cancelled();
        }
        self.calls
            .lock()
            .expect("fixture calls")
            .push(request.clone());
        if let Some(outcome) = self
            .outcomes
            .lock()
            .expect("fixture outcomes")
            .get(&request.backend_key)
            .cloned()
        {
            return outcome;
        }
        BackendOutcome::succeeded(self.default_result.lock().expect("fixture result").clone())
    }
}
