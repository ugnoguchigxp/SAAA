#![cfg(feature = "conversation-queue-e2e")]

#[tokio::test]
async fn legacy_and_stable_twenty_turn_queue_trials() {
    let legacy = saaa_lib::conversation_queue_e2e::run_context_trial("legacy")
        .await
        .expect("legacy baseline trial");
    let stable = saaa_lib::conversation_queue_e2e::run_context_trial("stable")
        .await
        .expect("stable candidate trial");
    assert_eq!(legacy["contextTrial"]["turns"], 20);
    assert!(legacy["contextTrial"]["fixedPrefixCount"].as_u64().unwrap() > 1);
    assert_eq!(stable["contextTrial"]["fixedPrefixCount"], 1);
    assert_eq!(stable["contextTrial"]["usageVerified"], true);
    assert_eq!(stable["contextTrial"]["fallbackVerified"], true);
    assert_eq!(stable["contextTrial"]["partialNotRetried"], true);
    assert_eq!(stable["contextTrial"]["invalidationVerified"], true);
    if let Ok(directory) = std::env::var("SAAA_CONTEXT_TRIAL_REPORT_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        for (mode, report) in [("legacy", legacy), ("stable", stable)] {
            let path = std::path::Path::new(&directory).join(format!("{mode}-requests.jsonl"));
            let rows = report["contextTrial"]["metrics"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| format!("{r}\n"))
                .collect::<String>();
            std::fs::write(path, rows).unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "real configured LARM; isolated DB; requires SAAA_LARM_CONTROL_URL and credential"]
async fn real_model_context_trial() {
    let directory =
        std::env::var("SAAA_CONTEXT_TRIAL_REPORT_DIR").expect("report directory required");
    for mode in std::env::var("SAAA_CONTEXT_LIVE_MODES")
        .unwrap_or_else(|_| "legacy,stable".into())
        .split(',')
    {
        let report = saaa_lib::conversation_queue_e2e::run_live_context_trial(mode)
            .await
            .expect("real context trial");
        let rows = report["metrics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>();
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join(format!("live-{mode}-requests.jsonl")),
            rows,
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join(format!("live-{mode}-turns.json")),
            serde_json::to_string_pretty(&report["answers"]).unwrap(),
        )
        .unwrap();
        assert_eq!(report["turns"], 20);
        let final_answer = report["answers"][19]["answer"].as_str().unwrap();
        assert!(
            final_answer.contains("音声"),
            "latest correction must survive: {final_answer}"
        );
        assert!(
            final_answer.to_lowercase().contains("profile")
                || final_answer.contains("プロフィール"),
            "deferred Profile must survive: {final_answer}"
        );
    }
}
