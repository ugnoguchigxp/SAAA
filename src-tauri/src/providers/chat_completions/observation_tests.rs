#![cfg(test)]
use super::*;
use serde_json::json;
use std::sync::Mutex;

#[derive(Default)]
struct Capture(Mutex<Vec<Value>>);
impl ObservationSink for Capture {
    fn sent(&self, _: &Value, _: &mut AttemptReceipt, _: Instant) {}
    fn finished(&self, receipt: &AttemptReceipt) {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::to_value(receipt).unwrap());
    }
}

#[test]
fn dropped_attempt_records_disconnection_without_retaining_request_text() {
    let sink = Capture::default();
    {
        let mut attempt = Attempt::new(
            Some(&sink),
            &json!({"stream":true,"model":"fixture","messages":[{"content":"private input"}]}),
        );
        attempt.content();
    }
    let rows = sink.0.lock().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["outcome"], "interrupted");
    assert_eq!(rows[0]["usageStatus"], "disconnected");
    assert!(
        rows[0]["firstContentMs"].as_u64().unwrap() <= rows[0]["completedMs"].as_u64().unwrap()
    );
    assert!(!rows[0].to_string().contains("private input"));
}

#[test]
fn dropped_attempt_preserves_explicit_provider_zero() {
    let sink = Capture::default();
    {
        let mut attempt = Attempt::new(Some(&sink), &json!({"stream":true}));
        let usage = crate::runtime::context::usage::parse_openai_usage(
            &json!({"prompt_tokens":12,"prompt_tokens_details":{"cached_tokens":0}}),
        );
        attempt.response(Some("concrete-model"), Some(&usage));
    }
    let rows = sink.0.lock().unwrap();
    assert_eq!(rows[0]["usageStatus"], "provider");
    assert_eq!(rows[0]["cacheReadTokens"], 0);
    assert_eq!(rows[0]["responseModel"], "concrete-model");
}
