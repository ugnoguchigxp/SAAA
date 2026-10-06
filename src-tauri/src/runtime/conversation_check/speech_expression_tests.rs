#![cfg(test)]
use super::*;

#[test]
fn one_response_controls_motion_and_voice_without_mutating_saved_voice() {
    let response = json!({"answers":{"motion":{"choice":"joyful"},"voice":{"choice":"excited"}}});
    let expression = parse(&response).unwrap();
    assert_eq!(expression.motion, "joyful");
    assert_eq!(expression.voice, SpeechExpression::Excited);
    assert!(!expression.fallback);
    let provider = crate::CloudTtsProviderSettings {
        id: "tts".into(),
        enabled: true,
        label: "voice".into(),
        location: "local".into(),
        endpoint: "http://127.0.0.1/v1".into(),
        model: "voicevox-core".into(),
        voice: "chosen".into(),
        response_format: "wav".into(),
        authentication: "none".into(),
        style: Some("normal".into()),
        speed: Some(1.0),
        pitch_scale: Some(0.0),
        intonation_scale: Some(1.0),
    };
    let excited =
        crate::voice::cloud_tts::speech_directive::apply_expression(&provider, expression.voice);
    let request = crate::voice::cloud_tts::speech_request::speech_request_value(
        &excited,
        "合格おめでとうございます！",
    );
    assert_eq!(request["input"], "合格おめでとうございます！");
    assert_eq!(request["voice"], "chosen");
    assert_eq!(request["style"], "normal");
    assert_eq!(request["speed"], 1.12);
    assert_eq!(request["pitch_scale"], 0.03);
    assert_eq!(request["intonation_scale"], 1.3);
    assert_eq!(provider.speed, Some(1.0));
    for bad in [
        json!({}),
        json!({"answers":{"motion":{"choice":"joyful"},"voice":{"choice":"invalid"}}}),
    ] {
        assert!(parse(&bad).is_err());
    }
}

#[tokio::test]
async fn timeout_falls_back_and_late_result_finishes_cleanup_without_applying() {
    let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = cleaned.clone();
    let task = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        flag.store(true, Ordering::Release);
        Ok(Expression {
            motion: "joyful".into(),
            voice: SpeechExpression::Excited,
            fallback: false,
        })
    });
    let result = await_decision(task, &RunCancellation::default(), 1)
        .await
        .unwrap();
    assert!(result.fallback);
    assert_eq!(result.voice, SpeechExpression::Natural);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(cleaned.load(Ordering::Acquire));
    assert_eq!(result.motion, "neutral");
}

#[tokio::test]
async fn cancellation_never_produces_a_speech_decision() {
    let cancellation = RunCancellation::default();
    cancellation.cancel();
    let task = tokio::spawn(async { Ok(Expression::default()) });
    assert_eq!(
        await_decision(task, &cancellation, 100).await.unwrap_err(),
        "Speech cancelled"
    );
}

#[tokio::test]
#[ignore = "requires real LARM/VOICEVOX and SAAA_LAYA_TEST_DB; synthesizes without playback"]
async fn laya_voicevox_synthesizes_expressive_chunks() {
    let db = std::env::var_os("SAAA_LAYA_TEST_DB").expect("read-only settings database");
    let db = rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap();
    let harness = crate::persistence::load_model_providers(&db)
        .unwrap()
        .harness;
    let credential = crate::providers::dynamic_lan::credential::load().unwrap();
    for (index, text) in [
        "やった！大成功です！本当にうれしいです！",
        "悲しいですね。つらい時は無理しないでくださいね。",
    ]
    .into_iter()
    .enumerate()
    {
        let mut result = laya::choose(
            &harness,
            credential.token(),
            &json!({"utterance":text}),
            laya::speech_question(),
            &|_| {},
        )
        .await;
        if result
            .as_ref()
            .err()
            .is_some_and(|e| e.contains("larm_catalog_unavailable"))
        {
            eprintln!("LARM catalog unavailable on first live connection; retrying once");
            result = laya::choose(
                &harness,
                credential.token(),
                &json!({"utterance":text}),
                laya::speech_question(),
                &|_| {},
            )
            .await;
        }
        let result = result.unwrap();
        let expression = parse(&result.response).unwrap();
        assert_ne!(expression.voice, SpeechExpression::Natural);
        let (_stop, cancellation) = tokio::sync::watch::channel(false);
        let session =
            saaa_larm_session::Session::connect_with_profile_credential_key_phase_and_providers(
                &harness.address,
                crate::providers::larm_resources::profile::preference(
                    harness.larm_profile.as_deref(),
                ),
                credential.token().to_string(),
                format!("saaa-expression-test-{}", uuid::Uuid::new_v4()),
                cancellation,
                None,
                Some(vec!["tts"]),
            )
            .await
            .map_err(|e| e.code)
            .unwrap();
        let audio = async {
            let lease = session.acquire("tts").await.map_err(str::to_string)?;
            let selected_voice = std::env::var("SAAA_LAYA_TEST_VOICE").ok();
            let test_voice = harness.tts_voice.as_deref().or(selected_voice.as_deref());
            let base = crate::providers::larm_resources::audio::tts_settings(
                lease.provider(),
                test_voice,
                Some(&harness),
            )?;
            assert_eq!(base.model, "voicevox-core");
            let provider = crate::voice::cloud_tts::speech_directive::apply_expression(
                &base,
                expression.voice,
            );
            let response = crate::voice::cloud_tts::request_audio_with_api_key(
                &provider,
                text,
                15_000,
                Arc::new(RunCancellation::default()),
                Some(lease.provider().token()),
            )
            .await?;
            crate::voice::cloud_tts::validate_audio_headers(&response, "wav")?;
            let bytes = response.bytes().await.map_err(|e| e.to_string())?;
            let mut decoder = crate::voice::http_audio::decode::Decoder::new("wav")?;
            let samples = decoder.push(&bytes)?;
            decoder.finish()?;
            assert!(samples.len() > 1000);
            eprintln!(
                "motion={} voice={} speed={:?} pitch={:?} intonation={:?} samples={}",
                expression.motion,
                expression.voice.audit_value(),
                provider.speed,
                provider.pitch_scale,
                provider.intonation_scale,
                samples.len()
            );
            if let Some(directory) = std::env::var_os("SAAA_LAYA_AUDIO_DIR") {
                std::fs::write(
                    std::path::Path::new(&directory).join(format!("laya-voice-{index}.wav")),
                    bytes,
                )
                .unwrap();
            }
            Ok::<(), String>(())
        }
        .await;
        session.close().await.unwrap();
        audio.unwrap();
    }
}
