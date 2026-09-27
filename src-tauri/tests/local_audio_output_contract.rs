#![cfg(feature = "conversation-queue-e2e")]

#[cfg(target_os = "macos")]
#[test]
fn macos_system_tts_wav_can_be_sent_to_local_pcm_output() {
    let directory = tempfile::tempdir().expect("private render directory");
    let path = directory.path().join("speech.wav");
    let status = std::process::Command::new("say")
        .arg("確認です。")
        .arg("-o")
        .arg(&path)
        .args(["--file-format=WAVE", "--data-format=LEI16@22050"])
        .status()
        .expect("macOS speech renderer starts");
    assert!(status.success(), "macOS speech renderer succeeds");
    let bytes = std::fs::read(path).expect("rendered WAV is readable");
    let (rate, channels, samples) = saaa_lib::conversation_queue_e2e::inspect_system_wav(&bytes)
        .expect("system WAV is complete PCM 16-bit audio");
    assert!(samples > 0);
    assert_eq!(rate, 22_050);
    assert_eq!(channels, 1);
}
