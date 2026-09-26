use crate::{
    config::{self, TtsSelection},
    credential, isolation, qwen, repository, speech,
};
use saaa_conversation_core::qwen::Event;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::{
    sync::watch,
    time::{Duration, Instant},
};

#[test]
#[ignore = "migrates an isolated Online Backup of the saved SAAA database"]
fn live_saved_database_copy_and_receipt() {
    let (mut db, _) = isolation::open_isolated_database().expect("isolated copy");
    repository::migrate(&db).expect("preview tables on copy");
    let config = config::load(&db).expect("saved settings");
    assert_eq!(
        config.fingerprint,
        isolation::source_config_fingerprint().unwrap()
    );
    repository::open_session(
        &mut db,
        "preview-copy-test",
        "conversation-copy-test",
        &config.fingerprint,
        "preview-copy-test-key",
        "now",
    )
    .expect("session on copy");
    assert!(repository::set_resource_state(
        &mut db,
        "preview-copy-test",
        "preparing",
        "ready",
        Some("copy-test-connection"),
        "now"
    )
    .unwrap());
    let receipt = repository::accept_input(
        &mut db,
        &saaa_conversation_core::contracts::TextInput {
            session_id: "preview-copy-test".into(),
            input_id: "input-copy-test".into(),
            text: "おはよう".into(),
        },
        "now",
    )
    .expect("receipt on copy");
    assert!(!receipt.duplicate);
}

#[tokio::test]
#[ignore = "connects to real LARM, requests Qwen, and plays selected TTS on the speaker"]
async fn live_one_qwen_and_selected_tts() {
    let (db, directory) = isolation::open_isolated_database().expect("isolated copy");
    let config = config::load(&db).expect("saved settings");
    let token = credential::load_larm_token().expect("saved LARM credential");
    let (_stop, receiver) = watch::channel(false);
    let (phase_tx, mut phase_rx) =
        watch::channel(saaa_larm_session::ConnectionPhase::ModelPreparing);
    let phase_log = tokio::spawn(async move {
        let mut previous = String::new();
        while phase_rx.changed().await.is_ok() {
            let phase = phase_rx.borrow_and_update().as_str().to_string();
            if phase != previous {
                eprintln!("LARM phase: {phase}");
                previous = phase;
            }
        }
    });
    let session = saaa_larm_session::Session::connect_with_profile_credential_key_and_phase(
        &config.harness_address,
        config.profile,
        token,
        format!("saaa-preview-live-{}", uuid::Uuid::new_v4().simple()),
        receiver,
        Some(phase_tx),
    )
    .await
    .expect("LARM create, poll, claim");
    phase_log.abort();
    let outcome = async {
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let (_cancel, cancel_rx) = watch::channel(false);
        let meta = qwen::run(
            &session,
            "おはよう",
            "preview-live-qwen",
            Instant::now() + Duration::from_secs(8),
            cancel_rx,
            move |event| {
                let observed = observed.clone();
                async move {
                    observed.lock().unwrap().push(event);
                    Ok(())
                }
            },
        )
        .await
        .map_err(|error| format!("Qwen {}: {}", error.stage, error.code))?;
        let body = {
            let events = events.lock().unwrap();
            events
                .iter()
                .filter_map(|event| match event {
                    Event::Body(text) => Some(text.as_str()),
                    Event::Control(_) => None,
                })
                .collect::<String>()
        };
        if body.trim().is_empty() {
            return Err("Qwen returned no public body".into());
        }
        eprintln!(
            "Qwen request={} connection={} allocation={} model={} text_bytes={}",
            meta.request_id,
            meta.connection_id,
            meta.allocation_id,
            meta.model,
            body.len()
        );
        let artifacts = directory.join("speech-artifacts");
        std::fs::create_dir_all(&artifacts).map_err(|_| "artifact directory")?;
        let (_speech_stop, mut speech_rx) = watch::channel(false);
        let clause = body
            .split_inclusive(['。', '！', '？', '.', '!', '?'])
            .next()
            .unwrap_or(body.as_str());
        let synthesis = speech::synthesize(
            &config.tts,
            &session,
            clause,
            &artifacts,
            &mut speech_rx,
            Instant::now() + Duration::from_secs(120),
            "preview-live-tts",
        )
        .await?;
        let started = Arc::new(AtomicBool::new(false));
        let mark = started.clone();
        speech::play(
            &synthesis.artifact,
            &mut speech_rx,
            Instant::now() + Duration::from_secs(120),
            move || {
                mark.store(true, Ordering::SeqCst);
                Ok(())
            },
        )
        .await?;
        if !started.load(Ordering::SeqCst) {
            return Err("player never consumed a sample".into());
        }
        eprintln!("TTS selected voice played: {}", clause);
        Ok::<_, String>(())
    }
    .await;
    let release = session.close().await;
    assert!(release.is_ok(), "LARM release must be confirmed");
    outcome.expect("live Qwen and TTS");
}

#[tokio::test]
#[ignore = "plays the saved System TTS voice on the real speaker without LARM"]
async fn live_system_tts_player_only() {
    let (db, directory) = isolation::open_isolated_database().expect("isolated copy");
    let config = config::load(&db).expect("saved settings");
    let TtsSelection::System { voice } = config.tts else {
        panic!("saved TTS is not System TTS");
    };
    let artifact = tempfile::Builder::new()
        .suffix(".aiff")
        .tempfile_in(&directory)
        .expect("private artifact");
    let mut command = tokio::process::Command::new("say");
    command.arg("-o").arg(artifact.path());
    if voice != "default" {
        command.arg("-v").arg(&voice);
    }
    assert!(command
        .arg("--")
        .arg("おはよう")
        .status()
        .await
        .expect("System TTS")
        .success());
    let (_stop, mut receiver) = watch::channel(false);
    let started = Arc::new(AtomicBool::new(false));
    let mark = started.clone();
    speech::play(
        artifact.path(),
        &mut receiver,
        Instant::now() + Duration::from_secs(120),
        move || {
            mark.store(true, Ordering::SeqCst);
            Ok(())
        },
    )
    .await
    .expect("real player drain");
    assert!(started.load(Ordering::SeqCst));
}
