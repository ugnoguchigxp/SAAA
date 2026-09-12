use super::*;
fn request() -> Request {
    serde_json::from_value(serde_json::json!({
        "schemaVersion": VERSION, "conversationId":"c1","turnId":"t1","requestId":"r1","contextRevision":1,
        "request":"比較してください","context":{"messages":[],"evidence":[],"truncated":false},
        "constraints":{"language":"ja","localOnly":true,"maxSpeechChars":240},"budget":{"timeoutMs":15000}
    })).unwrap()
}
fn answer() -> Answer {
    Answer {
        intent: Intent::Answer,
        speech_text: "条件を確認します。".into(),
        key_points: vec![],
        evidence_ids: vec![],
        limitations: vec![],
    }
}
#[test]
fn validates_correlation_and_utf8_character_limit() {
    let req = request();
    let mut a = answer();
    a.speech_text = "あ".repeat(240);
    let mut response = Response::bind(&req, a).unwrap();
    assert!(response.validate(&req).is_ok());
    response.speech_text.push('あ');
    assert!(response.validate(&req).is_err());
    response = Response::bind(&req, answer()).unwrap();
    response.context_revision += 1;
    assert!(response.validate(&req).is_err());
}
#[test]
fn rejects_unknown_fields_unavailable_evidence_and_cloud() {
    let mut req = request();
    let mut value = serde_json::to_value(&req).unwrap();
    value["model"] = "should-not-be-here".into();
    assert!(serde_json::from_value::<Request>(value).is_err());
    let mut a = answer();
    a.evidence_ids.push("fabricated".into());
    assert!(Response::bind(&req, a).is_err());
    req.constraints.local_only = false;
    assert!(req.validate().is_err());
    let mut value = serde_json::to_value(Response::bind(&request(), answer()).unwrap()).unwrap();
    value["canSpeakImmediately"] = true.into();
    assert!(serde_json::from_value::<Response>(value).is_err());
}
#[test]
fn byte_budget_is_independent_from_character_limit() {
    let mut req = request();
    for _ in 0..3 {
        req.context.messages.push(Message {
            role: Role::User,
            content: "あ".repeat(16000),
        });
    }
    assert!(req.validate().is_err());
    req.context.messages.clear();
    req.budget.timeout_ms = 15001;
    assert!(req.validate().is_err());
}
