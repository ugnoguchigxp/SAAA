//! Backend router. The kind is read from the DB-derived revision binding, never from model
//! arguments. A missing kind is treated as a legacy L-Lang binding only when it parses as one;
//! it is never silently routed to a remote server.

#![allow(private_interfaces)]

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use super::llang::LlangBinding;
use super::{BackendOutcome, BackendRequest, ToolBackend};
use crate::RunCancellation;

pub struct BackendRouter {
    pub(super) llang: Arc<dyn ToolBackend>,
    pub(super) mcp: Arc<dyn ToolBackend>,
    pub(super) records: Arc<dyn ToolBackend>,
}

impl BackendRouter {
    pub fn new(
        llang: Arc<dyn ToolBackend>,
        mcp: Arc<dyn ToolBackend>,
        records: Arc<dyn ToolBackend>,
    ) -> Self {
        Self {
            llang,
            mcp,
            records,
        }
    }

    pub fn kind(binding: &Value) -> &'static str {
        match binding.get("kind").and_then(Value::as_str) {
            Some("mcp_http") => "mcp_http",
            Some("llang") => "llang",
            Some("records") => "records",
            Some(_) => "unknown",
            None => {
                if LlangBinding::parse(binding).is_some() {
                    "llang"
                } else {
                    "unknown"
                }
            }
        }
    }
}

#[async_trait]
impl ToolBackend for BackendRouter {
    async fn invoke(
        &self,
        request: BackendRequest,
        cancellation: &RunCancellation,
    ) -> BackendOutcome {
        match Self::kind(&request.binding) {
            "llang" => self.llang.invoke(request, cancellation).await,
            "mcp_http" => self.mcp.invoke(request, cancellation).await,
            "records" => self.records.invoke(request, cancellation).await,
            // Unknown kinds and non-L-Lang bindings without a kind are refused before any send.
            _ => BackendOutcome::failed("integrity"),
        }
    }
}
