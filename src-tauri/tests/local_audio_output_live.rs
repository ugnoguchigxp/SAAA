#![cfg(feature = "conversation-queue-e2e")]

#[test]
#[ignore = "uses the current macOS output device; run explicitly on an operator workstation"]
fn exact_zero_samples_are_rendered_only_while_conversation_audio_is_idle() {
    let report = saaa_lib::conversation_queue_e2e::probe_idle_audio_output()
        .expect("local audio output probe starts");
    let samples = |name: &str| report[name]["renderedSamples"].as_u64().unwrap_or(0);
    assert_eq!(report["second"]["running"], true, "{report}");
    assert!(samples("second") > samples("first"), "{report}");
    assert_eq!(report["pausedAgain"]["speaking"], true, "{report}");
    assert_eq!(samples("pausedAgain"), samples("paused"), "{report}");
    assert!(samples("resumed") > samples("pausedAgain"), "{report}");
}

#[tokio::test]
#[ignore = "uses the current macOS output device; plays only zero-valued PCM"]
async fn system_tts_pcm_path_reaches_the_local_output_mixer() {
    assert!(
        saaa_lib::conversation_queue_e2e::probe_silent_pcm_playback()
            .await
            .expect("silent PCM playback succeeds")
    );
}

#[tokio::test]
#[ignore = "uses the current macOS output device; plays only zero-valued PCM"]
async fn vpio_capture_end_hands_remaining_pcm_to_local_output() {
    assert!(
        saaa_lib::conversation_queue_e2e::probe_vpio_handoff_to_silent_pcm()
            .await
            .expect("VPIO handoff reaches the output mixer")
    );
}
