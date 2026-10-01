#![cfg(feature = "conversation-queue-e2e")]

#[tokio::test]
async fn provider_failure_after_web_search_reaches_a_terminal_reply() {
    let report = saaa_lib::conversation_queue_e2e::run_failure_after_search()
        .await
        .expect("failed Ornith follow-up reaches a terminal reply");
    assert!(report["answer"]
        .as_str()
        .unwrap()
        .contains("結果を整理する段階で失敗"));
    // Terminal notices use the normal chunked, source-checked speech path.
    let spoken = report["spoken"].as_array().unwrap();
    assert!(!spoken.is_empty());
    assert_eq!(
        spoken
            .iter()
            .map(|chunk| chunk.as_str().unwrap())
            .collect::<String>(),
        report["answer"].as_str().unwrap()
    );
    assert_eq!(report["searches"], serde_json::json!(["fixture fact"]));
    assert_eq!(report["providerFailureKind"], "partial-output");
}
