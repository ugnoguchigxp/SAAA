//! Plays persisted progress messages through the shared speech output.
use super::*;

pub(super) async fn play_progress<R: tauri::Runtime>(
    state: &AppState,
    app: &tauri::AppHandle<R>,
    job: &crate::task_queue::Job,
    audit: &ConversationAudit,
) -> Result<(), String> {
    let _speech = SPEECH_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    ensure_current(state, job)?;
    if !super::queue_runtime::progress_eligible(state, job)? {
        return Ok(());
    }
    let text = serde_json::from_str::<serde_json::Value>(&job.payload)
        .ok()
        .and_then(|payload| payload["text"].as_str().map(str::to_string))
        .ok_or("待機案内の本文がありません。")?;
    let cancellation = Arc::new(RunCancellation::default());
    let _playback_state = PlaybackStateGuard::new(app, &job.key);
    *ACTIVE_SPEECH_CANCEL
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .map_err(|_| "音声の取消し状態を取得できません。")? =
        Some((job.key.clone(), SpeechPlaybackKind::Progress, cancellation.clone()));
    let _active = ActiveSpeechGuard;
    let (providers, route) = state.sqlite_readers.read(|connection| {
        Ok((
            persistence::load_model_providers(connection)?,
            persistence::load_routing_settings(connection)?.voice_speak,
        ))
    })?;
    ensure_current(state, job)?;
    if !super::queue_runtime::progress_eligible(state, job)? {
        return Ok(());
    }
    let mut continuous = None;
    let recorded = super::queue_runtime::record_progress_message(state, job, &text)?;
    if !recorded {
        return Ok(());
    }
    audit.text("tts", "conversation-tts-text", &text);
    audit.event(
        "tts",
        "conversation-tts-message",
        "decision",
        None,
        json!({"messageId":format!("progress_{}",job.id),"textBytes":text.len()}),
    );
    let _ = app.emit("conversation-queue-updated", ());
    play_chunk(
        app,
        &job.key,
        &providers,
        &route,
        &text,
        audit,
        cancellation,
        &mut continuous,
    )
    .await?;
    if let Some(player) = continuous {
        player.finish().await?;
    }
    Ok(())
}

fn ensure_current(state: &AppState, job: &crate::task_queue::Job) -> Result<(), String> {
    let current = state.sqlite_readers.read(|connection| {
        connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE id=?1 AND state='running' AND owner=?2 AND generation=?3)",
            params![job.id, job.owner, job.generation], |row| row.get::<_, bool>(0),
        ).map_err(database_error)
    })?;
    if current {
        Ok(())
    } else {
        Err("Speech cancelled".into())
    }
}

#[allow(clippy::too_many_arguments)]
async fn play_chunk<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    input_id: &str,
    providers: &crate::ModelProvidersSettings,
    route: &crate::VoiceRouteSettings,
    text: &str,
    audit: &ConversationAudit,
    cancellation: Arc<RunCancellation>,
    continuous: &mut Option<crate::voice::local_audio_output::ContinuousPlayback>,
) -> Result<(), String> {
    if cancellation.is_cancelled() {
        return Err("Speech cancelled".into());
    }
    let timeout = route.timeout_ms.min(120_000);
    let output = Arc::new(AtomicBool::new(false));
    let playback_audit = audit.clone();
    let playback_app = app.clone();
    let playback_id = input_id.to_string();
    let on_started = move || {
        set_speech_playback(&playback_app, &playback_id, true);
        playback_audit.event("tts", "conversation-tts-playback", "start", None, json!({}));
    };
    #[cfg(feature = "conversation-queue-e2e")]
    if crate::conversation_queue_e2e::capture_speech(text) {
        on_started();
        return Ok(());
    }
    if route.source == "harness" {
        let session = cached_larm_asr(providers, Some(audit)).await?;
        let request_started = std::time::Instant::now();
        let player = continuous.get_or_insert_with(|| {
            crate::voice::local_audio_output::ContinuousPlayback::start(
                cancellation.clone(),
                move || {
                    crate::providers::http_metrics::record(
                        "ttsRequestToFirstMixerSample",
                        request_started.elapsed(),
                    );
                    on_started();
                },
            )
        });
        return crate::voice::http_audio::play_larm_with_situation(
            &session,
            PRIMARY_CONVERSATION_ID,
            providers.harness.tts_voice.as_deref(),
            Some(&providers.harness),
            crate::voice::cloud_tts::speech_directive::SpeechExpression::Natural,
            output,
            text,
            timeout,
            cancellation,
            || {},
            None,
            Some(player),
        )
        .await;
    }
    let provider = providers
        .providers
        .iter()
        .find(|provider| route.provider_id.as_deref() == Some(provider.id()) && provider.enabled())
        .ok_or("設定済みのTTS Providerが見つかりません。")?;
    match provider {
        ModelProviderSettings::CloudTts(provider) => {
            let request_started = std::time::Instant::now();
            let player = continuous.get_or_insert_with(|| {
                crate::voice::local_audio_output::ContinuousPlayback::start(
                    cancellation.clone(),
                    move || {
                        crate::providers::http_metrics::record(
                            "ttsRequestToFirstMixerSample",
                            request_started.elapsed(),
                        );
                        on_started();
                    },
                )
            });
            crate::voice::http_audio::play_with_situation(
                provider,
                text,
                timeout,
                cancellation,
                output,
                || {},
                None,
                Some(player),
            )
            .await
        }
        ModelProviderSettings::SystemTts(provider) => {
            let player = continuous.get_or_insert_with(|| {
                crate::voice::local_audio_output::ContinuousPlayback::start(
                    cancellation.clone(),
                    on_started,
                )
            });
            let directory =
                tempfile::tempdir().map_err(|_| "TTS一時領域を作成できませんでした。")?;
            let path = crate::voice::system_tts::render_tts_artifact(
                text.to_string(),
                provider.voice.clone(),
                directory.path().to_path_buf(),
                cancellation.clone(),
            )
            .await?;
            if cancellation.is_cancelled() {
                return Err("Speech cancelled".into());
            }
            player.play_wav_file(&path, &cancellation).await
        }
        _ => Err("設定済みの音声出力ルートはTTS Providerではありません。".into()),
    }
}
