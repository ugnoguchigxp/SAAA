//! Plays only the user-facing Qwen response, in the order its text becomes speakable.
use super::*;
use crate::ipc_contract::RuntimeEvent;
use crate::runtime::event_hub::RuntimeEventSender;
use crate::voice::tts_chunker::{SelectReason, SentenceAccumulator};
use tokio::sync::mpsc;
#[path = "speech_projection.rs"]
mod speech_projection;
use speech_projection::SpeechProjection;

#[derive(Clone)]
pub(super) struct DeltaSink {
    sender: mpsc::UnboundedSender<String>,
    projection: Arc<std::sync::Mutex<SpeechProjection>>,
}

impl DeltaSink {
    pub(super) fn complete(&self, answer: &str) -> bool {
        let Ok(mut projection) = self.projection.lock() else {
            return false;
        };
        let Some(remainder) = projection.finish(answer) else {
            return false;
        };
        if !remainder.is_empty() {
            let _ = self.sender.send(remainder);
        }
        true
    }
}

impl RuntimeEventSender for DeltaSink {
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        if let RuntimeEvent::Delta { text, .. } = event {
            let Ok(mut projection) = self.projection.lock() else {
                return Ok(());
            };
            let safe = projection.append(&text);
            if projection.rejected() {
                return Err(tauri::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "音声用の回答に制御記号または上限超過を検出しました。",
                )));
            }
            // A closed audio worker does not invalidate an otherwise complete text response.
            if !safe.is_empty() {
                let _ = self.sender.send(safe);
            }
        }
        Ok(())
    }

    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }
}

pub(super) fn channel() -> (DeltaSink, mpsc::UnboundedReceiver<String>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    (
        DeltaSink {
            sender,
            projection: Arc::new(std::sync::Mutex::new(SpeechProjection::default())),
        },
        receiver,
    )
}

pub(super) async fn play<R: tauri::Runtime>(
    state: &AppState,
    app: &tauri::AppHandle<R>,
    speech_job: &crate::task_queue::Job,
    input_id: &str,
    audit: &ConversationAudit,
    cancellation: Arc<RunCancellation>,
    mut receiver: mpsc::UnboundedReceiver<String>,
) -> Result<usize, String> {
    let _speech = SPEECH_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let _playback_state = PlaybackStateGuard::new(app, input_id);
    *ACTIVE_SPEECH_CANCEL
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .map_err(|_| "音声の取消し状態を取得できません。")? =
        Some((input_id.to_string(), cancellation.clone()));
    let _active = ActiveSpeechGuard;
    let (providers, route) = state.sqlite_readers.read(|connection| {
        Ok((
            persistence::load_model_providers(connection)?,
            persistence::load_routing_settings(connection)?.voice_speak,
        ))
    })?;
    let mut accumulator = SentenceAccumulator::default();
    let mut full_text = String::new();
    let mut count = 0;
    loop {
        let delta = tokio::select! {
            biased;
            _ = cancellation.cancelled() => return Err("Speech cancelled".into()),
            delta = receiver.recv() => Some(delta),
            _ = tokio::time::sleep(std::time::Duration::from_millis(400)) => None,
        };
        if delta.is_none() {
            while let Some(chunk) = accumulator.next_chunk(SelectReason::Idle) {
                ensure_current(state, speech_job)?;
                play_chunk(
                    app,
                    input_id,
                    &providers,
                    &route,
                    &chunk.spoken,
                    audit,
                    cancellation.clone(),
                )
                .await?;
                count += 1;
            }
            continue;
        }
        let delta = delta.expect("idle case handled");
        let Some(delta) = delta else { break };
        ensure_current(state, speech_job)?;
        full_text.push_str(&delta);
        accumulator
            .append(&delta)
            .map_err(|_| "音声用本文が長すぎます。")?;
        while let Some(chunk) = accumulator.next_chunk(SelectReason::Append) {
            ensure_current(state, speech_job)?;
            play_chunk(
                app,
                input_id,
                &providers,
                &route,
                &chunk.spoken,
                audit,
                cancellation.clone(),
            )
            .await?;
            count += 1;
        }
    }
    // The final message may contain no sentence boundary. Flush it exactly once.
    accumulator
        .finish(&full_text)
        .map_err(|_| "音声用本文が一致しません。")?;
    while let Some(chunk) = accumulator.next_chunk(SelectReason::Completion) {
        ensure_current(state, speech_job)?;
        play_chunk(
            app,
            input_id,
            &providers,
            &route,
            &chunk.spoken,
            audit,
            cancellation.clone(),
        )
        .await?;
        count += 1;
    }
    if count == 0 {
        Err("読み上げ可能な回答本文がありません。".into())
    } else {
        Ok(count)
    }
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
            on_started,
            None,
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
            crate::voice::http_audio::play_with_situation(
                provider,
                text,
                timeout,
                cancellation,
                output,
                on_started,
                None,
            )
            .await
        }
        ModelProviderSettings::SystemTts(provider) => {
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
            let mut child = crate::voice::cloud_tts::spawn_audio_player(&path)?;
            on_started();
            loop {
                if let Some(status) = child
                    .try_wait()
                    .map_err(|_| "TTS再生を確認できませんでした。")?
                {
                    return if status.success() {
                        Ok(())
                    } else {
                        Err("TTS再生に失敗しました。".into())
                    };
                }
                tokio::select! {
                    _ = cancellation.cancelled() => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err("Speech cancelled".into());
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
                }
            }
        }
        _ => Err("設定済みの音声出力ルートはTTS Providerではありません。".into()),
    }
}
