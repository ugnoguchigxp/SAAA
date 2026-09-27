#![cfg(feature = "provider-unit-test-harness")]

/// Explicit opt-in: the normal screen's backend, saved settings and real LARM.
/// Input is 16 kHz mono signed little-endian PCM, not a mocked ASR response.
#[tokio::test]
#[ignore = "requires real LARM and SAAA_ASR_TEST_PCM / SAAA_ASR_TEST_DB"]
async fn saved_asr_route_transcribes_known_speech() {
    let database = std::env::var_os("SAAA_ASR_TEST_DB").expect("read-only settings database");
    let audio = std::env::var_os("SAAA_ASR_TEST_PCM").expect("16 kHz mono PCM fixture");
    let bytes = std::fs::read(audio).unwrap();
    assert_eq!(bytes.len() % 2, 0);
    let samples = bytes
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
        .collect::<Vec<_>>();
    assert!((1_600..=16_000 * 120).contains(&samples.len()));
    let result = saaa_lib::runtime::provider_unit_test::run_saved_asr_unit_test(
        std::path::Path::new(&database),
        &samples,
    )
    .await
    .expect("real connection, claim, transcription and release");
    println!("{}", serde_json::to_string(&result).unwrap());
    assert_eq!(result["capability"], "asr");
    let text = result["output"].as_str().unwrap();
    let expected = std::env::var("SAAA_ASR_EXPECTED_TEXT").expect("expected fixture phrase");
    assert!(
        text.contains(&expected),
        "expected known speech in ASR output"
    );
}
