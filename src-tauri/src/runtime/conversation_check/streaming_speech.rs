//! Plays persisted progress messages through the shared speech output.
use super::*;
use crate::voice::tts_chunker::{SelectReason, SentenceAccumulator};
use tauri::Manager;

pub(super) struct AnswerStreamReport {
    pub(super) started: bool,
    pub(super) error: Option<String>,
}

pub(super) async fn play_answer_stream<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input_id: String,
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<String>,
    job_cancellation: Arc<RunCancellation>,
    context_digest: String,
    audio_started: Arc<AtomicBool>,
) -> AnswerStreamReport {
    let cancellation = Arc::new(RunCancellation::default());
    let state = app.state::<AppState>();
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), input_id.clone());
    let mut accumulator = SentenceAccumulator::default();
    let mut player = None;
    let mut initialized = false;
    let mut started = false;
    let mut speech_guard = None;
    let mut active_guard = None;
    let mut playback_guard = None;
    let mut route: Option<super::voice_routes::PreparedVoice> = None;
    let mut dictionary = None;
    let mut purpose_attempt = None;
    let mut pending_spoken = String::new();
    let mut error = None;
    loop {
        let next = tokio::select! {
            biased;
            _=job_cancellation.cancelled()=>{cancellation.cancel(); error=Some("Speech cancelled".into());break;}
            _=cancellation.cancelled()=>{error=Some("Speech cancelled".into());break;}
            _=tokio::time::sleep_until(route.as_ref().map(|r|r.deadline).unwrap_or_else(||tokio::time::Instant::now()+std::time::Duration::from_secs(3600))), if initialized => {cancellation.cancel(); error=Some("この発話の全体期限を超えました".into());break;}
            next=receiver.recv()=>next,
        };
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
                if let Err(cause) =
                    super::queue_runtime::cancel_progress_for_stream(&state, &input_id)
                {
                    error = Some(cause);
                    break;
                }
                super::cancel_active_progress_speech(&input_id);
                speech_guard = Some(
                    SPEECH_LOCK
                        .get_or_init(|| tokio::sync::Mutex::new(()))
                        .lock()
                        .await,
                );
                let settings = state.sqlite_readers.read(|db| {
                    super::voice_routes::prepare(
                        db,
                        crate::providers::service_registry::Purpose::VoiceSpeak,
                    )
                });
                match settings {
                    Ok(prepared) => {
                        match direct_route::RouteAttempt::begin(&audit, &prepared.resolved) {
                            Ok(attempt) => purpose_attempt = Some(attempt),
                            Err(cause) => {
                                error = Some(cause);
                                break;
                            }
                        }
                        route = Some(prepared);
                        match state.tts_dictionary_cache.snapshot(&state.sqlite_readers) {
                            Ok(snapshot) => dictionary = Some(snapshot),
                            Err(cause) => {
                                error = Some(cause);
                                break;
                            }
                        }
                    }
                    Err(cause) => {
                        error = Some(cause);
                        break;
                    }
                }
                *ACTIVE_SPEECH_CANCEL
                    .get_or_init(|| std::sync::Mutex::new(None))
                    .lock()
                    .expect("speech cancellation registry") = Some((
                    input_id.clone(),
                    SpeechPlaybackKind::Answer,
                    cancellation.clone(),
                ));
                active_guard = Some(ActiveSpeechGuard);
                playback_guard = Some(PlaybackStateGuard::new(&app, &input_id));
                initialized = true;
            }
            pending_spoken.push_str(&chunk.spoken);
            let matcher = dictionary.as_ref().expect("speech dictionary loaded");
            let ready_len = matcher.ready_prefix_len(&pending_spoken);
            if ready_len == 0 {
                continue;
            }
            let ready = pending_spoken[..ready_len].to_string();
            pending_spoken.drain(..ready_len);
            let prepared = route.as_ref().expect("speech route loaded");
            let budget = match prepared.validate(&state) {
                Ok(budget) => budget,
                Err(cause) => {
                    error = Some(cause);
                    cancellation.cancel();
                    break;
                }
            };
            let mut voice_route = prepared.route.clone();
            voice_route.timeout_ms = budget;
            let providers = &prepared.providers;
            let playback = play_chunk(
                &app,
                &input_id,
                &state,
                &prepared.resolved,
                providers,
                &voice_route,
                &ready,
                matcher,
                &audit,
                cancellation.clone(),
                &mut player,
            );
            let played = tokio::select! {
                _=job_cancellation.cancelled()=>{ cancellation.cancel(); Err("Speech cancelled".into()) },
                result=playback=>result,
                _=tokio::time::sleep_until(prepared.deadline)=> { cancellation.cancel(); Err("この発話の全体期限を超えました".into()) }
            };
            started |= super::speech_playing();
            if started {
                audio_started.store(true, Ordering::Release);
            }
            if let Err(cause) = played {
                error = Some(cause);
                break;
            }
        }
        if error.is_some() || next.is_none() {
            break;
        }
    }
    if error.is_none() && !pending_spoken.is_empty() {
        let current_context = super::queue_runtime::stream_context_digest(&state, &input_id);
        if current_context.as_deref() != Ok(context_digest.as_str()) {
            error = Some("回答の根拠が変更されたため音声を中止しました。".into());
        } else {
            let prepared = route.as_ref().expect("pending speech has a route");
            let mut voice_route = prepared.route.clone();
            match prepared.validate(&state) {
                Ok(budget) => voice_route.timeout_ms = budget,
                Err(cause) => {
                    error = Some(cause);
                    cancellation.cancel();
                }
            }
            let providers = &prepared.providers;
            let playback = play_chunk(
                &app,
                &input_id,
                &state,
                &prepared.resolved,
                providers,
                &voice_route,
                &pending_spoken,
                dictionary.as_ref().expect("speech dictionary loaded"),
                &audit,
                cancellation.clone(),
                &mut player,
            );
            let result = tokio::select! {
                _=job_cancellation.cancelled()=>{ cancellation.cancel(); Err("Speech cancelled".into()) },
                result=playback=>result,
                _=tokio::time::sleep_until(prepared.deadline)=>{ cancellation.cancel(); Err("この発話の全体期限を超えました".into()) }
            };
            if let Err(cause) = result {
                error = Some(cause);
            }
            started |= super::speech_playing();
            if started {
                audio_started.store(true, Ordering::Release);
            }
        }
    }
    if error.is_some() {
        cancellation.cancel();
    }
    if let Some(player) = player {
        let finished = tokio::select! {
            _=job_cancellation.cancelled()=>{cancellation.cancel();Err("Speech cancelled".into())},
            _=tokio::time::sleep_until(route.as_ref().expect("initialized voice").deadline)=>{cancellation.cancel();Err("この発話の全体期限を超えました".into())},
            result=player.finish()=>result,
        };
        if let Err(cause) = finished {
            error.get_or_insert(cause);
        }
    }
    if let Some(attempt) = &mut purpose_attempt {
        if let Err(cause) = attempt.finish(error.is_none()) {
            error.get_or_insert(cause);
        }
    }
    drop(playback_guard);
    drop(active_guard);
    drop(speech_guard);
    AnswerStreamReport { started, error }
}

#[path = "streaming_progress.rs"]
mod progress;
pub(super) use progress::play_progress;

#[path = "speech_playback.rs"]
mod playback;
use playback::play_chunk;
