#![cfg(feature = "conversation-queue-e2e")]

#[tokio::test]
#[ignore = "uses public search engines and fetches returned pages"]
async fn public_search_results_can_be_read() {
    let result = saaa_lib::conversation_queue_e2e::run_live_web_retrieval()
        .await
        .expect("live web retrieval");
    assert!(result["hits"].as_u64().unwrap() > 0);
    assert!(result["textCharacters"].as_u64().unwrap() > 0);
    eprintln!("{result}");
}
