use serde_json::{json, Value};
fn object(properties: Value) -> Value {
    let required: Vec<_> = properties
        .as_object()
        .expect("schema properties")
        .keys()
        .cloned()
        .collect();
    json!({"type":"object","additionalProperties":false,"required":required,"properties":properties})
}
fn string(max: usize) -> Value {
    json!({"type":"string","minLength":1,"maxLength":max})
}
fn array(items: Value, max: usize) -> Value {
    json!({"type":"array","items":items,"maxItems":max})
}
pub fn answer() -> Value {
    object(json!({
        "intent":{"type":"string","enum":["answer","clarify","insufficient_context"]},
        "speechText":string(240),"keyPoints":array(string(160),5),
        "evidenceIds":array(string(160),8),"limitations":array(string(160),5)
    }))
}
pub fn output() -> Value {
    let mut props = answer()["properties"].clone();
    let p = props.as_object_mut().expect("properties");
    p.insert("schemaVersion".into(), json!({"const":super::VERSION}));
    for name in ["conversationId", "turnId", "requestId"] {
        p.insert(name.into(), string(160));
    }
    p.insert(
        "contextRevision".into(),
        json!({"type":"integer","minimum":1}),
    );
    object(props)
}
pub fn input() -> Value {
    object(json!({
        "schemaVersion":{"const":super::VERSION},"conversationId":string(160),
        "turnId":string(160),"requestId":string(160),"contextRevision":{"type":"integer","minimum":1},
        "request":string(16000),
        "context":object(json!({
            "messages":array(object(json!({"role":{"enum":["user","assistant"]},"content":string(16000)})),32),
            "evidence":array(object(json!({"id":string(160),"source":string(512),"content":string(16000),"world":{"anyOf":[{"type":"null"},object(json!({"schemaVersion":{"const":super::world::VERSION},"instructionAuthority":{"const":"none"},"focusScopeKey":{"anyOf":[{"type":"null"},string(512)]},"allowedScopeKeys":{"type":"array","items":string(512),"minItems":1,"maxItems":64,"uniqueItems":true},"scopeDigest":string(128),"sourceKinds":{"type":"array","items":{"enum":["situation","coding","delegation","schedule"]},"maxItems":4,"uniqueItems":true},"capturedAtMs":{"type":"integer"},"expiresAtMs":{"type":"integer"}}))]}})),8),
            "truncated":{"type":"boolean"}
        })),
        "constraints":object(json!({"language":{"enum":["ja","en","auto"]},"localOnly":{"const":true},"maxSpeechChars":{"type":"integer","minimum":1,"maximum":240}})),
        "budget":object(json!({"timeoutMs":{"type":"integer","minimum":1,"maximum":super::TIMEOUT_MS}}))
    }))
}
pub fn tool() -> Value {
    json!({"name":super::TOOL,"description":"Answer from supplied conversation context only. Local inference; no tool execution.",
        "inputSchema":input(),"outputSchema":output()})
}
