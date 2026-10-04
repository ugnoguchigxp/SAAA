//! Bounded transport receipts. Never retain request text, headers or raw usage.
use crate::runtime::context::usage::ProviderUsage;
use serde::Serialize;
use serde_json::Value;
use std::time::Instant;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttemptReceipt {
    pub(crate) schema_version: u8,
    pub(crate) http_attempt_id: String,
    pub(crate) request_mode: &'static str,
    pub(crate) requested_model: String,
    pub(crate) response_model: Option<String>,
    pub(crate) wire_bytes: usize,
    pub(crate) messages_bytes: usize,
    pub(crate) fixed_prefix_bytes: usize,
    pub(crate) fixed_prefix_digest: String,
    pub(crate) message_prefix_bytes: Option<usize>,
    pub(crate) prefix_change_reason: &'static str,
    pub(crate) input_tokens: Option<u64>,
    pub(crate) cache_read_tokens: Option<u64>,
    pub(crate) output_tokens: Option<u64>,
    pub(crate) usage_status: &'static str,
    pub(crate) first_content_ms: Option<u64>,
    pub(crate) completed_ms: u64,
    pub(crate) outcome: &'static str,
}

pub(crate) trait ObservationSink: Send + Sync {
    fn sent(&self, body: &Value, receipt: &mut AttemptReceipt, started: Instant);
    fn finished(&self, receipt: &AttemptReceipt);
}

pub(super) struct Attempt<'a> {
    sink: Option<&'a dyn ObservationSink>,
    started: Instant,
    receipt: AttemptReceipt,
}

pub(crate) fn digest(bytes: impl AsRef<[u8]>) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

impl<'a> Attempt<'a> {
    pub(super) fn new(sink: Option<&'a dyn ObservationSink>, body: &Value) -> Self {
        let started = Instant::now();
        let fixed = body["messages"][0]["content"].as_str().unwrap_or_default();
        let mut receipt = AttemptReceipt {
            schema_version: 1,
            http_attempt_id: uuid::Uuid::new_v4().simple().to_string(),
            request_mode: if body["stream"] == true {
                "sse"
            } else {
                "json"
            },
            requested_model: body["model"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(256)
                .collect(),
            response_model: None,
            wire_bytes: body.to_string().len(),
            messages_bytes: body["messages"].to_string().len(),
            fixed_prefix_bytes: fixed.len(),
            fixed_prefix_digest: digest(fixed),
            message_prefix_bytes: None,
            prefix_change_reason: "first-request",
            input_tokens: None,
            cache_read_tokens: None,
            output_tokens: None,
            usage_status: "missing",
            first_content_ms: None,
            completed_ms: 0,
            outcome: "interrupted",
        };
        if let Some(sink) = sink {
            sink.sent(body, &mut receipt, started);
        }
        Self {
            sink,
            started,
            receipt,
        }
    }

    pub(super) fn content(&mut self) {
        self.receipt
            .first_content_ms
            .get_or_insert(self.started.elapsed().as_millis() as u64);
    }

    pub(super) fn response(&mut self, model: Option<&str>, usage: Option<&ProviderUsage>) {
        self.receipt.response_model = model.filter(|m| m.len() <= 256).map(str::to_string);
        if let Some(usage) = usage {
            self.receipt.usage_status = "provider";
            self.receipt.input_tokens = usage.input_tokens;
            self.receipt.cache_read_tokens = usage.cache_read_tokens;
            self.receipt.output_tokens = usage.output_tokens;
        }
    }

    pub(super) fn finish(&mut self, result: &Result<String, super::ProviderAttemptError>) {
        self.receipt.outcome = match result {
            Ok(_) => "success",
            Err(super::ProviderAttemptError::Cancelled { .. }) => "cancelled",
            Err(_) => "failure",
        };
        if result.is_err() && self.receipt.usage_status == "missing" {
            self.receipt.usage_status = "disconnected";
        }
    }
}

impl Drop for Attempt<'_> {
    fn drop(&mut self) {
        if self.receipt.outcome == "interrupted" && self.receipt.usage_status == "missing" {
            self.receipt.usage_status = "disconnected";
        }
        self.receipt.completed_ms = self.started.elapsed().as_millis() as u64;
        if let Some(sink) = self.sink {
            sink.finished(&self.receipt);
        }
    }
}

#[cfg(test)]
#[path = "observation_tests.rs"]
mod tests;
