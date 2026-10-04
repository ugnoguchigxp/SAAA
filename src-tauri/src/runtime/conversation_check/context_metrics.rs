//! Privacy-preserving comparison of complete messages, not tokenizer/cache claims.
use super::{context_compiler::ContextStep, ConversationAudit};
use crate::providers::chat_completions::observation::{digest, AttemptReceipt, ObservationSink};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::Instant;

type Signatures = Vec<(String, usize)>;
static PREFIXES: OnceLock<Mutex<VecDeque<(String, Signatures)>>> = OnceLock::new();

pub(super) fn clear() {
    if let Some(prefixes) = PREFIXES.get() {
        if let Ok(mut prefixes) = prefixes.lock() {
            prefixes.clear();
        }
    }
}

#[derive(Clone)]
pub(super) struct RequestMetrics {
    audit: ConversationAudit,
    logical_id: String,
    step: usize,
    mode: &'static str,
    tool_digest: String,
    connection: String,
    latest: Arc<Mutex<Option<(String, Instant, bool)>>>,
    retry_blocked: Arc<AtomicBool>,
}

impl RequestMetrics {
    pub(super) fn new(
        audit: &ConversationAudit,
        context: &ContextStep<'_>,
        connection: &str,
        retry_blocked: Arc<AtomicBool>,
    ) -> Self {
        Self {
            retry_blocked,
            audit: audit.clone(),
            logical_id: uuid::Uuid::new_v4().simple().to_string(),
            step: context.step,
            mode: context.mode.name(),
            tool_digest: context.fixed.tool_set_digest.clone(),
            connection: digest(connection),
            latest: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn for_provider(&self, endpoint: &str) -> Self {
        let mut resolved = self.clone();
        resolved.connection = digest(format!("{}:{}", self.connection, endpoint));
        resolved
    }

    pub(super) fn compiled(&self, omitted: usize) {
        self.audit.event("conversation", "conversation-context-request", "request", None,
            json!({"schemaVersion":1,"correlationId":self.audit.correlation_id,"logicalRequestId":self.logical_id,"step":self.step,
                "mode":self.mode,"toolSetDigest":self.tool_digest,"serializerVersion":1,"omissionCount":omitted}));
    }

    pub(super) fn invalidated(&self) {
        self.retry_blocked.store(true, Ordering::Release);
        clear();
    }

    pub(super) fn visible(&self) {
        self.retry_blocked.store(true, Ordering::Release);
        if let Ok(mut latest) = self.latest.lock() {
            if let Some((attempt, started, visible)) = latest.as_mut() {
                if !*visible {
                    *visible = true;
                    self.audit.event("conversation", "conversation-context-visible", "progress", None,
                        json!({"schemaVersion":1,"correlationId":self.audit.correlation_id,"logicalRequestId":self.logical_id,
                            "httpAttemptId":attempt,"firstVisibleMs":started.elapsed().as_millis() as u64}));
                }
            }
        }
    }
}

impl ObservationSink for RequestMetrics {
    fn sent(&self, body: &Value, receipt: &mut AttemptReceipt, started: Instant) {
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some((receipt.http_attempt_id.clone(), started, false));
        }
        let mut configuration = body.clone();
        if let Some(object) = configuration.as_object_mut() {
            object.remove("messages");
            object.remove("stream");
        }
        let key = digest(format!(
            "{}:{}:{}",
            self.connection, self.mode, configuration
        ));
        let signatures: Signatures = body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|message| {
                let bytes = message.to_string();
                (digest(&bytes), bytes.len())
            })
            .collect();
        if signatures.len() > 256 {
            receipt.prefix_change_reason = "comparison-capacity";
            return;
        }
        if let Ok(mut prefixes) = PREFIXES.get_or_init(|| Mutex::new(VecDeque::new())).lock() {
            if let Some(index) = prefixes.iter().position(|(prior, _)| prior == &key) {
                if let Some((_, previous)) = prefixes.remove(index) {
                    let matched: usize = previous
                        .iter()
                        .zip(&signatures)
                        .take_while(|(a, b)| a.0 == b.0)
                        .map(|(_, b)| b.1)
                        .sum();
                    receipt.message_prefix_bytes = Some(matched);
                    receipt.prefix_change_reason = if previous == signatures {
                        "unchanged"
                    } else if previous.first().map(|v| &v.0) != signatures.first().map(|v| &v.0) {
                        "fixed-prefix-changed"
                    } else {
                        "history-or-runtime"
                    };
                }
            }
            prefixes.push_back((key, signatures));
            while prefixes.len() > 16 {
                prefixes.pop_front();
            }
        }
    }

    fn finished(&self, receipt: &AttemptReceipt) {
        let mut value = serde_json::to_value(receipt).unwrap_or(Value::Null);
        if let Some(object) = value.as_object_mut() {
            object.insert("correlationId".into(), json!(self.audit.correlation_id));
            object.insert("logicalRequestId".into(), json!(self.logical_id));
            object.insert("step".into(), json!(self.step));
            object.insert("mode".into(), json!(self.mode));
            object.insert("connectionIdentity".into(), json!(self.connection));
        }
        self.audit.event(
            "conversation",
            "conversation-context-attempt",
            "terminal",
            Some(receipt.outcome),
            value,
        );
        if receipt.outcome != "success" {
            clear();
        }
    }
}
