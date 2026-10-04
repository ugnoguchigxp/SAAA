//! Wire-level assertions for explicitly configured compatibility options.
use serde_json::{json, Value};
pub(super) fn validate(body: &Value) {
    assert_eq!(body["max_completion_tokens"], 4096);
    assert!(body["max_tokens"].is_null());
    assert_eq!(body["stream"], false);
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    assert!(body["reasoning_effort"].is_null());
}

pub(super) fn normalize_native(body: &mut Value) {
    assert_eq!(body["stream"], false);
    assert!(body["system"].as_str().is_some_and(|v| !v.is_empty()));
    let instruction = body["system"].clone();
    body["messages"]
        .as_array_mut()
        .expect("native messages")
        .insert(0, json!({"role":"system","content":instruction}));
}
