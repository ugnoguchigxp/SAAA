use super::streaming_speech::*;

pub(crate) async fn play_progress<R: tauri::Runtime>(
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
        .map_err(|_| "音声の取消し状態を取得できません。")? = Some((
        job.key.clone(),
        SpeechPlaybackKind::Progress,
        cancellation.clone(),
    ));
    let _active = ActiveSpeechGuard;
    let availability = crate::providers::service_registry::LocalAvailability::of(state);
    let prepared = state.sqlite_readers.read(|db| {
        super::super::voice_routes::prepare(
            db,
            crate::providers::service_registry::Purpose::VoiceSpeak,
            availability,
        )
    })?;
    let providers = &prepared.providers;
    let mut route = prepared.route.clone();
    route.timeout_ms = prepared.validate(state)?;
    let mut purpose_attempt = direct_route::RouteAttempt::begin(audit, &prepared.resolved)?;
    let dictionary = state.tts_dictionary_cache.snapshot(&state.sqlite_readers)?;
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
    tokio::select! {
        _=tokio::time::sleep_until(prepared.deadline)=>{cancellation.cancel();return Err("この発話の全体期限を超えました".into());},
        result=async {
    play_chunk(
        app,
        &job.key,
        state,
        &prepared.resolved,
        providers,
        &route,
        &text,
        &dictionary,
        audit,
        cancellation.clone(),
        &mut continuous,
    )
    .await?;

    if let Some(player) = continuous {
        player.finish().await?;
    }
            Ok::<(),String>(())
        }=>result?,
    }
    purpose_attempt.finish(true)?;
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
