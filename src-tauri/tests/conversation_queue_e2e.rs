#![cfg(feature = "conversation-queue-e2e")]

#[test]
fn unfinished_two_model_job_migrates_on_an_isolated_database() {
    saaa_lib::conversation_queue_e2e::verify_legacy_queue_migration()
        .expect("legacy queue migration");
}

#[tokio::test]
async fn asr_ornith_tool_saved_answer_tts_reaches_terminal_queue_states() {
    let report = saaa_lib::conversation_queue_e2e::run()
        .await
        .expect("conversation queue E2E");
    assert_eq!(report["transcript"], "今日の事実を調べて");
    assert!(report["answer"]
        .as_str()
        .unwrap()
        .contains("[出典1: example.invalid](https://example.invalid/report)"));
    assert_eq!(
        report["spoken"],
        serde_json::json!(["資料では確認済みの事実は42です。"])
    );
    assert!(report["progressSpoken"].is_null() || report["progressSpoken"] == "只今お調べします。");
    assert_eq!(report["speechBeforeDone"], true);
    assert_eq!(report["quickGreeting"], "こんにちは。");
    assert_eq!(report["cancellationVerified"], true);
    let waiting = saaa_lib::conversation_queue_e2e::run_waiting()
        .await
        .expect("ten-second waiting E2E");
    assert!(waiting["waitingSpoken"].is_null());
    let rejected = saaa_lib::conversation_queue_e2e::run_invalid_reply()
        .await
        .expect("invalid answer is neither saved nor spoken");
    assert_eq!(rejected["rejectedWithoutSpeech"], true);
    let authentication = saaa_lib::conversation_queue_e2e::run_authentication_failure()
        .await
        .expect("authentication failure evicts the rejected session without retrying it");
    assert_eq!(authentication["reconnected"], true);
    assert_eq!(authentication["llmCalls"], 2);
}

#[test]
fn dictionary_tools_enforce_scope_confirmation_conflict_cancellation_and_warm_cache() {
    let report = saaa_lib::conversation_queue_e2e::verify_tts_dictionary_tools()
        .expect("dictionary tool contract");
    assert_eq!(report["cases"], 15);
    assert_eq!(report["warmCacheWithoutDictionaryTable"], true);
}

#[tokio::test]
#[ignore = "real configured LARM model; isolated dictionary; requires SAAA_LARM_CONTROL_URL and LARM_API_TOKEN"]
async fn live_model_pronunciation_corrections_and_confirmation() {
    let report = saaa_lib::conversation_queue_e2e::run_live_tts_dictionary()
        .await
        .expect("live dictionary acceptance");
    println!("{report}");
    assert_eq!(report["cases"].as_array().unwrap().len(), 9);
}
