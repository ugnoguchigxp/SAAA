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
    let cloud = saaa_lib::conversation_queue_e2e::run_cloud_conversation()
        .await
        .expect("cloud conversation E2E");
    // The bounded tool loop (search, failed fetch, fetch, answer) runs on the cloud route.
    assert!(cloud["answer"]
        .as_str()
        .unwrap()
        .starts_with("資料では確認済みの事実は42です。"));
    assert_eq!(cloud["larmRequests"], 0, "{cloud}");
    assert_eq!(cloud["llmCalls"], 4);
    let native = saaa_lib::conversation_queue_e2e::run_cloud_boundary("native")
        .await
        .expect("native Messages conversation and tool loop");
    assert!(native["answer"].is_string(), "{native}");
    assert_eq!(native["llmCalls"], 4, "{native}");
    assert_eq!(native["larmRequests"], 0, "{native}");
    let options = saaa_lib::conversation_queue_e2e::run_cloud_boundary("options")
        .await
        .expect("JSON-only provider preserves explicit compatibility options across tool steps");
    assert!(options["answer"].is_string(), "{options}");
    assert_eq!(options["llmCalls"], 4, "{options}");
    assert_eq!(options["larmRequests"], 0, "{options}");
    let revoked = saaa_lib::conversation_queue_e2e::run_cloud_boundary("revoked")
        .await
        .expect("cloud revocation E2E");
    assert!(revoked["answer"].is_null(), "{revoked}");
    assert_eq!(revoked["llmCalls"], 1, "{revoked}");
    assert_eq!(revoked["larmRequests"], 0, "{revoked}");
    let consent = saaa_lib::conversation_queue_e2e::run_cloud_boundary("consent")
        .await
        .expect("cloud permission withdrawal");
    assert!(consent["answer"].is_null(), "{consent}");
    assert_eq!(consent["llmCalls"], 1, "{consent}");
    let fallback = saaa_lib::conversation_queue_e2e::run_cloud_boundary("fallback")
        .await
        .expect("initial rejection fallback");
    assert!(fallback["answer"].is_string(), "{fallback}");
    assert_eq!(fallback["llmCalls"], 4, "{fallback}");
    assert_eq!(
        fallback["usage"][0]["resourceId"], "res:svc-cloud-llm",
        "{fallback}"
    );
    assert_eq!(fallback["usage"][0]["status"], "accepted", "{fallback}");
    assert_eq!(
        fallback["usage"][0]["matchesCurrentSettings"], true,
        "fallback belongs to the current binding: {fallback}"
    );
    let auth = saaa_lib::conversation_queue_e2e::run_cloud_boundary("auth")
        .await
        .expect("authentication never falls back");
    assert!(auth["answer"].is_null(), "{auth}");
    assert_eq!(auth["llmCalls"], 0, "{auth}");
    let switched = saaa_lib::conversation_queue_e2e::run_cloud_boundary("switch")
        .await
        .expect("ordinary route changes apply to next job");
    assert!(switched["answer"].is_string(), "{switched}");
    assert_eq!(switched["llmCalls"], 4, "{switched}");
    assert_eq!(switched["larmRequests"], 0, "{switched}");
    assert_eq!(
        switched["usage"][0]["matchesCurrentSettings"], false,
        "{switched}"
    );
    let deadline = saaa_lib::conversation_queue_e2e::run_cloud_boundary("deadline")
        .await
        .expect("cloud deadline E2E");
    assert!(deadline["answer"].is_null(), "{deadline}");
    assert!(deadline["elapsedMs"].as_u64().unwrap() < 1800, "{deadline}");
    assert!(deadline["llmCalls"].as_u64().unwrap() < 4, "{deadline}");
    // The fixture server is process-global, so the worker scenario runs in sequence here.
    let worker = saaa_lib::conversation_queue_e2e::worker::run_worker()
        .await
        .expect("worker-mode conversation E2E");
    assert!(
        worker["answer"]
            .as_str()
            .is_some_and(|answer| answer.contains("42")),
        "{worker}"
    );
    assert_eq!(worker["task"]["state"], "succeeded", "{worker}");
    assert_eq!(worker["task"]["delivery"], "sync_delivered", "{worker}");
    assert_eq!(worker["conversationSawOffer"], true, "{worker}");
    assert_eq!(worker["conversationSawWorkerResult"], true, "{worker}");
    // The conversation agent never sees raw search/page text or the injected hit.
    assert_eq!(worker["rawTextReachedConversation"], false, "{worker}");
    assert_eq!(worker["injectionReachedWorker"], false, "{worker}");
    assert!(worker["checkerCalls"].as_u64().unwrap() >= 1, "{worker}");
    assert_eq!(worker["terminalAudits"], 1, "{worker}");
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
