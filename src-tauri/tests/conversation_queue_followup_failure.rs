#![cfg(feature = "conversation-queue-e2e")]

#[tokio::test]
async fn provider_failure_after_web_search_reaches_a_terminal_reply() {
    let report = saaa_lib::conversation_queue_e2e::run_failure_after_search()
        .await
        .expect("failed Ornith follow-up reaches Qwen and speech");
    assert!(report["answer"]
        .as_str()
        .unwrap()
        .contains("結果を整理する段階で失敗"));
    assert_eq!(report["spoken"], serde_json::json!([report["answer"]]));
    assert_eq!(report["searches"], serde_json::json!(["fixture fact"]));
    assert_eq!(
        report["providerFailureDetail"],
        "chat-finish-reason-not-stop"
    );
}
