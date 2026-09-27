#![cfg(feature = "conversation-queue-e2e")]

#[tokio::test]
async fn asr_qwen_ornith_tool_qwen_tts_reaches_terminal_queue_states() {
    let report = saaa_lib::conversation_queue_e2e::run()
        .await
        .expect("conversation queue E2E");
    assert_eq!(report["transcript"], "今日の事実を調べて");
    assert_eq!(report["answer"], "調査結果は42です。");
    assert_eq!(report["spoken"], serde_json::json!(["調査結果は42です。"]));
    assert_eq!(report["speechBeforeDone"], true);
    assert_eq!(report["quickGreeting"], "こんにちは。");
    assert_eq!(report["cancellationVerified"], true);
}
