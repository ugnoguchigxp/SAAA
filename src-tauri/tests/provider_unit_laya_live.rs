#![cfg(feature = "provider-unit-test-harness")]
/// Same backend as the unit-test screen; reads settings without changing them.
#[tokio::test]
#[ignore = "requires real LARM and SAAA_LAYA_TEST_DB"]
async fn saved_laya_route_selects_japanese_actions() {
    let db = std::env::var_os("SAAA_LAYA_TEST_DB").expect("read-only settings database");
    for (text, expected) in [
        ("左に移動して", "left"),
        ("右に移動して", "right"),
        ("ジャンプして", "jump"),
        ("止まって", "stop"),
    ] {
        let result = saaa_lib::runtime::provider_unit_test::run_saved_laya_unit_test(
            std::path::Path::new(&db),
            text,
        )
        .await
        .unwrap();
        let response: serde_json::Value =
            serde_json::from_str(result["output"].as_str().unwrap()).unwrap();
        println!(
            "input={text} choice={} confidence={} elapsed_ms={}",
            response["answers"]["action"]["choice"],
            response["answers"]["action"]["answer_confidence"],
            result["latencyMs"]
        );
        assert_eq!(response["answers"]["action"]["choice"], expected);
    }
}

#[tokio::test]
#[ignore = "requires real LARM and SAAA_LAYA_TEST_DB"]
async fn saved_laya_selects_avatar_expressions() {
    let db = std::env::var_os("SAAA_LAYA_TEST_DB").expect("read-only settings database");
    for (text, expected, voice) in [
        (
            "おはようございます！今日もよろしくお願いします。",
            "greeting",
            "bright",
        ),
        (
            "やった！大成功です！本当にうれしいです！",
            "joyful",
            "excited",
        ),
        (
            "悲しいですね。つらい時は無理しないでくださいね。",
            "downcast",
            "gentle",
        ),
        ("その通りですね。私も同じ意見です。", "agreeing", "natural"),
    ] {
        let response = saaa_lib::runtime::provider_unit_test::run_saved_laya_avatar_test(
            std::path::Path::new(&db),
            text,
        )
        .await
        .unwrap();
        println!(
            "expected={expected} choice={} confidence={}",
            response["answers"]["motion"]["choice"],
            response["answers"]["motion"]["answer_confidence"]
        );
        assert_eq!(response["answers"]["motion"]["choice"], expected);
        assert_eq!(response["answers"]["voice"]["choice"], voice);
    }
}
