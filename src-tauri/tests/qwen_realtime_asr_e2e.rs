#![cfg(feature = "provider-unit-test-harness")]

#[tokio::test]
async fn qwen_live_session_streams_audio_and_completes_a_transcript() {
    saaa_lib::runtime::provider_unit_test::verify_qwen_local_websocket_fixture().await;
}

#[tokio::test]
async fn qwen_rejects_invalid_session_configuration_before_capture() {
    saaa_lib::runtime::provider_unit_test::verify_qwen_rejected_session_fixture().await;
}

#[tokio::test]
async fn conversation_gate_keeps_target_audio_and_zeros_other_speakers_without_shortening_time() {
    saaa_lib::runtime::provider_unit_test::verify_conversation_speaker_gate_fixture().await;
}
