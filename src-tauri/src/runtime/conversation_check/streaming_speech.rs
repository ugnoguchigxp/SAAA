//! Plays persisted progress messages through the shared speech output.
use super::*;
use tauri::Manager;
use crate::voice::tts_chunker::{SelectReason, SentenceAccumulator};

pub(super) struct AnswerStreamReport {
    pub(super) started: bool,
    pub(super) error: Option<String>,
}

pub(super) async fn play_answer_stream<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input_id: String,
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<String>,
    cancellation: Arc<RunCancellation>,
    context_digest: String,
    audio_started: Arc<AtomicBool>,
) -> AnswerStreamReport {
    let state = app.state::<AppState>();
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), input_id.clone());
    let mut accumulator = SentenceAccumulator::default();
    let mut player = None;
    let mut initialized = false;
    let mut started = false;
    let mut speech_guard = None;
    let mut active_guard = None;
    let mut playback_guard = None;
    let mut route = None;
    let mut dictionary_entries = Vec::new();
    let mut pending_spoken = String::new();
    let mut error = None;
    loop {
        let next = receiver.recv().await;
        let reason = if let Some(delta) = next.as_deref() {
            if accumulator.append(delta).is_err() {
                error = Some("回答の音声用テキストが長すぎます。".into());
                break;
            }
            SelectReason::Append
        } else {
            accumulator.finish_current();
            SelectReason::Completion
        };
        while let Some(chunk) = accumulator.next_chunk(reason) {
            if cancellation.is_cancelled() {
                error = Some("Speech cancelled".into());
                break;
            }
            let current_context = super::queue_runtime::stream_context_digest(&state, &input_id);
            if current_context.as_deref() != Ok(context_digest.as_str()) {
                error = Some("回答の根拠が変更されたため音声を中止しました。".into());
                break;
            }
            if !initialized {
                if let Err(cause) = super::queue_runtime::cancel_progress_for_stream(&state, &input_id) {
                    error = Some(cause);
                    break;
                }
                super::cancel_active_progress_speech(&input_id);
                speech_guard = Some(SPEECH_LOCK.get_or_init(|| tokio::sync::Mutex::new(())).lock().await);
                let settings = state.sqlite_readers.read(|connection| {
                    Ok((persistence::load_model_providers(connection)?,
                        persistence::load_routing_settings(connection)?.voice_speak,
                        crate::tts_dictionary::list(connection)?))
                });
                match settings {
                    Ok((providers, voice_route, entries)) => {
                        route = Some((providers, voice_route));
                        dictionary_entries = entries;
                    }
                    Err(cause) => { error = Some(cause); break; }
                }
                *ACTIVE_SPEECH_CANCEL
                    .get_or_init(|| std::sync::Mutex::new(None))
                    .lock().expect("speech cancellation registry") =
                    Some((input_id.clone(), SpeechPlaybackKind::Answer, cancellation.clone()));
                active_guard = Some(ActiveSpeechGuard);
                playback_guard = Some(PlaybackStateGuard::new(&app, &input_id));
                initialized = true;
            }
            pending_spoken.push_str(&chunk.spoken);
            let ready_len = crate::tts_dictionary::ready_stream_prefix_len(
                &pending_spoken,
                &dictionary_entries,
            );
            if ready_len == 0 {
                continue;
            }
            let ready = pending_spoken[..ready_len].to_string();
            pending_spoken.drain(..ready_len);
            let (providers, voice_route) = route.as_ref().expect("speech route loaded");
            let played = play_chunk(&app, &input_id, providers, voice_route,
                &ready, &audit, cancellation.clone(), &mut player).await;
            started |= super::speech_playing();
            if started { audio_started.store(true, Ordering::Release); }
            if let Err(cause) = played {
                error = Some(cause);
                break;
            }
        }
        if error.is_some() || next.is_none() { break; }
    }
    if error.is_none() && !pending_spoken.is_empty() {
        let current_context = super::queue_runtime::stream_context_digest(&state, &input_id);
        if current_context.as_deref() != Ok(context_digest.as_str()) {
            error = Some("回答の根拠が変更されたため音声を中止しました。".into());
        } else {
            let (providers, voice_route) = route.as_ref().expect("pending speech has a route");
            if let Err(cause) = play_chunk(&app, &input_id, providers, voice_route,
                &pending_spoken, &audit, cancellation.clone(), &mut player).await {
                error = Some(cause);
            }
            started |= super::speech_playing();
            if started { audio_started.store(true, Ordering::Release); }
        }
    }
    if let Some(player) = player {
        if let Err(cause) = player.finish().await { error.get_or_insert(cause); }
    }
    drop(playback_guard);
    drop(active_guard);
    drop(speech_guard);
    AnswerStreamReport { started, error }
}

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
    let spoken = app.state::<AppState>().sqlite_readers.read(|connection| {
        crate::tts_dictionary::apply_saved(connection, text)
    })?;
    if spoken.trim().is_empty() {
        return Ok(());
    }
    audit.text("tts", "conversation-tts-provider-text", &spoken);
    let text = spoken.as_str();
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
