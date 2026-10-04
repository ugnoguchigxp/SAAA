//! Conversation-screen turn path: finalized ASR text, Ornith answer and TTS.
//! Continuous partial-ASR routing and the durable work queue remain separate runtime work.
#[path = "conversation_check/context_compiler.rs"]
mod context_compiler;
#[path = "conversation_check/context_metrics.rs"]
mod context_metrics;
mod direct_route;
pub(crate) mod queue_runtime;
#[path = "conversation_check/queue_tools.rs"]
mod queue_tools;
#[path = "conversation_check/streaming_speech.rs"]
mod streaming_speech;
pub(crate) fn spawn_queue_workers<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    queue_runtime::spawn(app);
}
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::time::Instant;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;

use crate::{
    database_error, now_iso, persistence, validate_identifier, AppState, ModelProviderSettings,
    RunCancellation, StartTurnInput, PRIMARY_CONVERSATION_ID,
};

static ASR_SESSION: OnceLock<tokio::sync::Mutex<Option<CachedAsrSession>>> = OnceLock::new();
static SPEECH_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpeechPlaybackKind {
    Answer,
    Progress,
}
type ActiveSpeechCancellation = (String, SpeechPlaybackKind, Arc<RunCancellation>);
static ACTIVE_SPEECH_CANCEL: OnceLock<std::sync::Mutex<Option<ActiveSpeechCancellation>>> =
    OnceLock::new();
const MAX_ANSWER_BYTES: usize = 64 * 1024;
const SOURCE_LINKS_MARKER: &str = "\n\n<!-- saaa:source-links -->\n";
static ACTIVE_SPEECH_PLAYBACK: OnceLock<std::sync::Mutex<Option<String>>> = OnceLock::new();

fn speech_playing() -> bool {
    ACTIVE_SPEECH_PLAYBACK
        .get()
        .and_then(|slot| slot.lock().ok())
        .is_some_and(|slot| slot.is_some())
}

fn set_speech_playback<R: tauri::Runtime>(app: &tauri::AppHandle<R>, input_id: &str, active: bool) {
    let slot = ACTIVE_SPEECH_PLAYBACK.get_or_init(|| std::sync::Mutex::new(None));
    let Ok(mut current) = slot.lock() else {
        return;
    };
    if active {
        if current.as_deref() == Some(input_id) {
            return;
        }
        *current = Some(input_id.to_string());
    } else {
        if current.as_deref() != Some(input_id) {
            return;
        }
        *current = None;
    }
    drop(current);
    let _ = app.emit("conversation-queue-updated", ());
}

struct PlaybackStateGuard<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    input_id: String,
}

impl<R: tauri::Runtime> PlaybackStateGuard<R> {
    fn new(app: &tauri::AppHandle<R>, input_id: &str) -> Self {
        Self {
            app: app.clone(),
            input_id: input_id.to_string(),
        }
    }
}

impl<R: tauri::Runtime> Drop for PlaybackStateGuard<R> {
    fn drop(&mut self) {
        set_speech_playback(&self.app, &self.input_id, false);
    }
}

struct ActiveSpeechGuard;
impl Drop for ActiveSpeechGuard {
    fn drop(&mut self) {
        if let Some(slot) = ACTIVE_SPEECH_CANCEL.get() {
            if let Ok(mut current) = slot.lock() {
                *current = None;
            }
        }
    }
}

fn cancel_active_speech(input_id: &str) {
    if let Some(slot) = ACTIVE_SPEECH_CANCEL.get() {
        if let Ok(current) = slot.lock() {
            if let Some((id, _, cancellation)) = current.as_ref() {
                if id == input_id {
                    cancellation.cancel();
                }
            }
        }
    }
}

fn cancel_active_progress_speech(input_id: &str) {
    if let Some(slot) = ACTIVE_SPEECH_CANCEL.get() {
        if let Ok(current) = slot.lock() {
            if let Some((id, kind, cancellation)) = current.as_ref() {
                if id == input_id && *kind == SpeechPlaybackKind::Progress {
                    cancellation.cancel();
                }
            }
        }
    }
}

struct CachedAsrSession {
    route: String,
    session: Arc<saaa_larm_session::Session>,
}

fn asr_session() -> &'static tokio::sync::Mutex<Option<CachedAsrSession>> {
    ASR_SESSION.get_or_init(|| tokio::sync::Mutex::new(None))
}

#[cfg(feature = "conversation-queue-e2e")]
pub(crate) async fn reset_fixture_asr_session() {
    context_metrics::clear();
    if let Some(cached) = asr_session().lock().await.take() {
        let _ = cached.session.close().await;
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SubmitInput {
    input_id: String,
    text: String,
    source: CheckSource,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CheckSource {
    Configured,
    Larm,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SubmitResult {
    content: String,
    model: String,
    provider_label: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationStageEvent {
    stage: &'static str,
}

fn report_stage(channel: &tauri::ipc::Channel<ConversationStageEvent>, stage: &'static str) {
    let _ = channel.send(ConversationStageEvent { stage });
}

/// One correlation ID follows an utterance from capture through the spoken reply.
/// Audit writes are best effort and contain no credentials or audio samples.
#[derive(Clone)]
struct ConversationAudit {
    writer: Arc<persistence::SqliteWriter>,
    correlation_id: String,
}

impl ConversationAudit {
    fn new(writer: Arc<persistence::SqliteWriter>, correlation_id: String) -> Self {
        Self {
            writer,
            correlation_id,
        }
    }

    fn event(
        &self,
        component: &str,
        event_name: &str,
        phase: &str,
        outcome: Option<&str>,
        attributes: Value,
    ) {
        let mut attributes = attributes.to_string();
        if attributes.len() > 2_048 {
            let bytes = attributes.len();
            self.text(component, "conversation-audit-overflow", &attributes);
            attributes =
                json!({ "overflow": true, "sourceEvent": event_name, "bytes": bytes }).to_string();
        }
        if let Err(error) = self.write_event(component, event_name, phase, outcome, &attributes) {
            eprintln!("Conversation audit write failed for {event_name}: {error}");
        }
    }

    fn write_event(
        &self,
        component: &str,
        event_name: &str,
        phase: &str,
        outcome: Option<&str>,
        attributes: &str,
    ) -> Result<(), String> {
        self.writer.write(|connection| {
            connection.execute(
                "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,correlation_id,conversation_id,attributes_json) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    format!("audit_{}", uuid::Uuid::new_v4().simple()),
                    now_iso(), component, event_name, phase, outcome,
                    self.correlation_id, PRIMARY_CONVERSATION_ID, attributes,
                ],
            ).map_err(database_error)?;
            Ok(())
        })
    }

    fn text(&self, component: &str, event_name: &str, value: &str) {
        if std::env::var("SAAA_CONVERSATION_TEXT_AUDIT").as_deref() != Ok("1") {
            use sha2::{Digest, Sha256};
            self.event(component, event_name, "progress", None,
                json!({"bytes":value.len(),"sha256":format!("{:x}",Sha256::digest(value.as_bytes())),"textRecorded":false}));
            return;
        }
        let mut parts = Vec::new();
        let mut start = 0;
        while start < value.len() {
            // Keep even JSON-escaped control characters below audit_events' 2 KiB limit.
            let mut end = (start + 256).min(value.len());
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            parts.push(&value[start..end]);
            start = end;
        }
        if parts.is_empty() {
            parts.push("");
        }
        let text_id = format!("text_{}", uuid::Uuid::new_v4().simple());
        for (index, part) in parts.iter().enumerate() {
            self.event(
                component,
                event_name,
                "progress",
                None,
                json!({
                    "textId": text_id, "part": index + 1, "parts": parts.len(), "text": part,
                    "bytes": value.len(),
                }),
            );
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CaptureAuditInput {
    event_name: String,
    phase: String,
    outcome: Option<String>,
    correlation_id: Option<String>,
    attributes: Value,
}

#[tauri::command]
pub(crate) fn record_conversation_capture_audit_event(
    state: tauri::State<'_, AppState>,
    input: CaptureAuditInput,
) -> Result<(), String> {
    if !input.event_name.starts_with("conversation-asr-")
        || input.event_name.len() > 80
        || !input
            .event_name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        || !matches!(
            input.phase.as_str(),
            "request" | "start" | "state" | "progress" | "decision" | "terminal" | "error"
        )
        || input
            .outcome
            .as_deref()
            .is_some_and(|value| !matches!(value, "success" | "failure" | "degraded"))
        || !input.attributes.is_object()
    {
        return Err("会話ASR監査イベントが不正です。".into());
    }
    let correlation_id = input
        .correlation_id
        .unwrap_or_else(|| "conversation-capture".into());
    validate_identifier(&correlation_id, "correlation id")?;
    let attributes = input.attributes.to_string();
    if attributes.len() > 2_048 {
        return Err("会話ASR監査イベントが大きすぎます。".into());
    }
    ConversationAudit::new(state.sqlite_writer.clone(), correlation_id).write_event(
        "voice-asr",
        &input.event_name,
        &input.phase,
        input.outcome.as_deref(),
        &attributes,
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TranscribeInput {
    audio_upload_id: String,
    utterance_id: String,
    kind: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TranscribeResult {
    text: String,
    language: Option<String>,
    provider_label: String,
}

#[tauri::command]
pub(crate) async fn transcribe_conversation_audio(
    state: tauri::State<'_, AppState>,
    input: TranscribeInput,
) -> Result<TranscribeResult, String> {
    validate_identifier(&input.utterance_id, "utterance id")?;
    if !matches!(input.kind.as_str(), "partial" | "final") {
        return Err("ASR更新種別が不正です。".into());
    }
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), input.utterance_id.clone());
    let started = Instant::now();
    let kind = input.kind.clone();
    audit.event(
        "voice-asr",
        "conversation-asr-request",
        "request",
        None,
        json!({
            "kind": input.kind, "audioUploadId": input.audio_upload_id,
        }),
    );
    let result = transcribe_conversation_audio_inner(&state, input, &audit).await;
    match &result {
        Ok(value) => {
            audit.event(
                "voice-asr",
                "conversation-asr-result",
                "terminal",
                Some("success"),
                json!({
                    "kind": kind,
                    "provider": value.provider_label, "language": value.language,
                    "elapsedMs": started.elapsed().as_millis() as u64,
                    "textBytes": value.text.len(),
                }),
            );
            audit.text("voice-asr", "conversation-asr-text", &value.text);
        }
        Err(error) => audit.event(
            "voice-asr",
            "conversation-asr-result",
            "error",
            Some("failure"),
            json!({
                "kind": kind,
                "error": error, "elapsedMs": started.elapsed().as_millis() as u64,
            }),
        ),
    }
    result
}

async fn transcribe_conversation_audio_inner(
    state: &AppState,
    input: TranscribeInput,
    audit: &ConversationAudit,
) -> Result<TranscribeResult, String> {
    let samples = state
        .audio_uploads
        .consume(&input.audio_upload_id, "conversation-asr")?;
    if samples.len() < 1_600 || samples.len() > 16_000 * 120 {
        return Err("録音は0.1秒以上、2分以内にしてください。".into());
    }
    let mut gate = crate::voice::conversation_speaker::prepare(state)?;
    audit.event(
        "voice-asr",
        "conversation-speaker-gate",
        "decision",
        None,
        json!({"scope":gate.scope()}),
    );
    let samples = crate::voice::conversation_speaker::filter_samples(&mut gate, samples).await;
    if !samples.iter().any(|sample| sample.abs() > 0.0001) {
        return Err("ASR_NO_SPEECH: 本人の発話を検出できませんでした。".into());
    }
    // Record identity and signal level, not raw microphone audio, so repeated
    // transcripts can be distinguished from duplicate uploads and silent input.
    {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        let mut energy = 0.0_f64;
        for sample in &samples {
            digest.update(sample.to_le_bytes());
            energy += f64::from(*sample).powi(2);
        }
        audit.event(
            "voice-asr",
            "conversation-asr-audio",
            "request",
            None,
            json!({"kind":input.kind,"samples":samples.len(),
                "rms":(energy / samples.len() as f64).sqrt(),
                "sha256":format!("{:x}",digest.finalize())}),
        );
    }
    let (providers, route) = state.sqlite_readers.read(|connection| {
        Ok((
            persistence::load_model_providers(connection)?,
            persistence::load_routing_settings(connection)?.voice_transcribe,
        ))
    })?;
    let cancellation = Arc::new(RunCancellation::default());
    audit.event(
        "voice-asr",
        "conversation-asr-route",
        "decision",
        None,
        json!({
            "source": route.source, "timeoutMs": route.timeout_ms,
            "providerId": route.provider_id,
        }),
    );
    let (text, language, provider_label) = if route.source == "harness" {
        let timeout = std::time::Duration::from_millis(route.timeout_ms.min(120_000));
        let session = tokio::time::timeout(timeout, cached_larm_asr(&providers, Some(audit)))
            .await
            .map_err(|_| {
                "LARMのProvider接続準備が時間内に完了せず、ASRへ音声を送れませんでした。"
                    .to_string()
            })??;
        let result = tokio::time::timeout(timeout, async {
            let lease = session.acquire("asr").await.map_err(str::to_string)?;
            if lease.provider().protocol != "openai.audio-transcriptions.v1" {
                return Err("選択済みLARMのASRプロトコルに対応していません。".into());
            }
            audit.event(
                "provider",
                "conversation-asr-provider",
                "start",
                None,
                json!({
                    "role": "asr", "model": lease.provider().model,
                    "protocol": lease.provider().protocol,
                }),
            );
            let budget = lease.request_budget(timeout).map_err(str::to_string)?;
            let provider = crate::providers::larm_resources::audio::asr_settings(lease.provider());
            crate::voice::cloud_asr::transcribe_full(
                &provider,
                &samples,
                16_000,
                budget.as_millis() as u64,
                cancellation,
                Some(lease.provider().token()),
            )
            .await
        })
        .await
        .map_err(|_| "ASRの文字起こしが時間内に完了しませんでした。".to_string());
        let reset_session = match &result {
            Err(_) => true,
            Ok(Err(error)) => !error.starts_with("ASR_NO_SPEECH:"),
            Ok(Ok(_)) => false,
        };
        if reset_session {
            audit.event(
                "voice-asr",
                "conversation-asr-session-reset",
                "decision",
                None,
                json!({
                    "reason": "recognition-failed",
                }),
            );
            release_conversation_asr_session().await?;
        }
        let (text, language) = result??;
        (text, language, "LARM ASR".to_string())
    } else {
        let provider = providers
            .providers
            .iter()
            .find(|candidate| {
                route.provider_id.as_deref() == Some(candidate.id()) && candidate.enabled()
            })
            .ok_or("設定済みのASR Providerが見つかりません。")?;
        let ModelProviderSettings::CloudAsr(provider) = provider else {
            return Err("設定済みの音声入力ルートはASR Providerではありません。".into());
        };
        let (text, language) = crate::voice::cloud_asr::transcribe_full(
            provider,
            &samples,
            16_000,
            route.timeout_ms.min(120_000),
            cancellation,
            None,
        )
        .await?;
        (text, language, provider.label.clone())
    };
    Ok(TranscribeResult {
        text,
        language,
        provider_label,
    })
}

#[cfg(feature = "conversation-queue-e2e")]
pub(crate) async fn transcribe_fixture_audio(
    state: &AppState,
    input_id: &str,
) -> Result<String, String> {
    let samples = vec![1_000_i16; 3_200];
    let audio_upload_id = state.audio_uploads.stage_pcm_for_e2e(&samples);
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), input_id.into());
    transcribe_conversation_audio_inner(
        state,
        TranscribeInput {
            audio_upload_id,
            utterance_id: input_id.into(),
            kind: "final".into(),
        },
        &audit,
    )
    .await
    .map(|result| result.text)
}

#[tauri::command]
pub(crate) async fn release_conversation_asr_session() -> Result<(), String> {
    let previous = asr_session().lock().await.take();
    if let Some(previous) = previous {
        if Arc::strong_count(&previous.session) == 1 {
            previous
                .session
                .close()
                .await
                .map_err(|_| "LARMのASR接続を解放できませんでした。".to_string())?;
        }
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn speak_conversation_answer(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    input_id: String,
) -> Result<(), String> {
    validate_identifier(&input_id, "input id")?;
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), input_id.clone());
    let started = Instant::now();
    audit.event(
        "tts",
        "conversation-tts-request",
        "request",
        None,
        json!({}),
    );
    let result = speak_conversation_answer_inner(&state, &app, &input_id, &audit, None).await;
    match &result {
        Ok(()) => audit.event(
            "tts",
            "conversation-tts-result",
            "terminal",
            Some("success"),
            json!({
                "elapsedMs": started.elapsed().as_millis() as u64,
            }),
        ),
        Err(error) => audit.event(
            "tts",
            "conversation-tts-result",
            "error",
            Some("failure"),
            json!({
                "error": error, "elapsedMs": started.elapsed().as_millis() as u64,
            }),
        ),
    }
    result
}

async fn speak_conversation_answer_inner<R: tauri::Runtime>(
    state: &AppState,
    app: &tauri::AppHandle<R>,
    input_id: &str,
    audit: &ConversationAudit,
    speech_job: Option<&crate::task_queue::Job>,
) -> Result<(), String> {
    let _speech = SPEECH_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    // Validate after acquiring the player: queued audio may have waited while its
    // source was forgotten or changed. Replays must pass the same boundary.
    if let Some(job) = speech_job {
        queue_runtime::validate_speech_context(state, job)?;
    }
    let _playback_state = PlaybackStateGuard::new(app, input_id);
    let answer_id = format!("reply_{input_id}");
    let (content, providers, route) = state.sqlite_readers.read(|connection| {
        let content: String = connection.query_row(
            "SELECT content FROM conversation_messages WHERE id=?1 AND conversation_id=?2 AND role='assistant'",
            params![answer_id, PRIMARY_CONVERSATION_ID], |row| row.get(0),
        ).map_err(database_error)?;
        Ok((content, persistence::load_model_providers(connection)?,
            persistence::load_routing_settings(connection)?.voice_speak))
    })?;
    if content.trim().is_empty() || content.len() > MAX_ANSWER_BYTES {
        return Err("読み上げる回答がありません。".into());
    }
    let dictionary = state.tts_dictionary_cache.snapshot(&state.sqlite_readers)?;
    let spoken = dictionary.apply(&speech_text_for_answer(&content));
    if spoken.trim().is_empty() {
        return Ok(());
    }
    audit.text("tts", "conversation-tts-text", &spoken);
    audit.event(
        "tts",
        "conversation-tts-message",
        "decision",
        None,
        json!({"messageId":answer_id,"textBytes":spoken.len()}),
    );
    audit.event(
        "tts",
        "conversation-tts-route",
        "decision",
        None,
        json!({
            "source": route.source, "providerId": route.provider_id,
            "timeoutMs": route.timeout_ms, "textBytes": content.len(),
        }),
    );
    let cancellation = Arc::new(RunCancellation::default());
    *ACTIVE_SPEECH_CANCEL
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .map_err(|_| "音声の取消し状態を取得できません。")? = Some((
        input_id.to_string(),
        SpeechPlaybackKind::Answer,
        cancellation.clone(),
    ));
    let _active_speech = ActiveSpeechGuard;
    if let Some(job) = speech_job {
        let current = state.sqlite_readers.read(|connection| {
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE id=?1 AND state='running' AND owner=?2 AND generation=?3)",
                params![job.id,job.owner,job.generation],
                |row| row.get::<_, bool>(0),
            ).map_err(database_error)
        })?;
        if !current || cancellation.is_cancelled() {
            return Err("音声の仕事は中止されました。".into());
        }
    }
    #[cfg(feature = "conversation-queue-e2e")]
    if crate::conversation_queue_e2e::capture_speech(&spoken) {
        return Ok(());
    }
    let output = Arc::new(AtomicBool::new(false));
    if route.source == "harness" {
        let session = cached_larm_asr(&providers, Some(audit)).await?;
        audit.event(
            "tts",
            "conversation-tts-provider",
            "start",
            None,
            json!({
                "role": "tts", "profile": providers.harness.larm_profile,
                "voice": providers.harness.tts_voice,
            }),
        );
        let playback_audit = audit.clone();
        let playback_app = app.clone();
        let playback_id = input_id.to_string();
        let result = crate::voice::http_audio::play_larm_with_situation(
            &session,
            PRIMARY_CONVERSATION_ID,
            providers.harness.tts_voice.as_deref(),
            Some(&providers.harness),
            crate::voice::cloud_tts::speech_directive::SpeechExpression::Natural,
            output,
            &spoken,
            route.timeout_ms.min(120_000),
            cancellation,
            move || {
                set_speech_playback(&playback_app, &playback_id, true);
                playback_audit.event("tts", "conversation-tts-playback", "start", None, json!({}));
            },
            None,
            None,
        )
        .await;
        return result;
    }
    let provider = providers
        .providers
        .iter()
        .find(|provider| route.provider_id.as_deref() == Some(provider.id()) && provider.enabled())
        .ok_or("設定済みのTTS Providerが見つかりません。")?;
    match provider {
        ModelProviderSettings::CloudTts(provider) => {
            audit.event(
                "tts",
                "conversation-tts-provider",
                "start",
                None,
                json!({
                    "model": provider.model, "providerId": provider.id,
                }),
            );
            let playback_audit = audit.clone();
            let playback_app = app.clone();
            let playback_id = input_id.to_string();
            crate::voice::http_audio::play_with_situation(
                provider,
                &spoken,
                route.timeout_ms.min(120_000),
                cancellation,
                output,
                move || {
                    set_speech_playback(&playback_app, &playback_id, true);
                    playback_audit.event(
                        "tts",
                        "conversation-tts-playback",
                        "start",
                        None,
                        json!({}),
                    )
                },
                None,
                None,
            )
            .await
        }
        ModelProviderSettings::SystemTts(provider) => {
            audit.event(
                "tts",
                "conversation-tts-provider",
                "start",
                None,
                json!({
                    "providerId": provider.id, "mode": "system",
                }),
            );
            let playback_audit = audit.clone();
            let playback_app = app.clone();
            let playback_id = input_id.to_string();
            let player = crate::voice::local_audio_output::ContinuousPlayback::start(
                cancellation.clone(),
                move || {
                    set_speech_playback(&playback_app, &playback_id, true);
                    playback_audit.event(
                        "tts",
                        "conversation-tts-playback",
                        "start",
                        None,
                        json!({}),
                    );
                },
            );
            let directory =
                tempfile::tempdir().map_err(|_| "TTS一時領域を作成できませんでした。")?;
            let path = crate::voice::system_tts::render_tts_artifact(
                spoken,
                provider.voice.clone(),
                directory.path().to_path_buf(),
                cancellation.clone(),
            )
            .await?;
            audit.event(
                "tts",
                "conversation-tts-render",
                "terminal",
                Some("success"),
                json!({}),
            );
            player.play_wav_file(&path, &cancellation).await?;
            player.finish().await
        }
        _ => Err("設定済みの音声出力ルートはTTS Providerではありません。".into()),
    }
}

fn speech_text_for_answer(content: &str) -> String {
    let answer = content
        .split_once(SOURCE_LINKS_MARKER)
        .map_or(content, |(answer, _)| answer);
    crate::voice_text::text_for_speech(answer)
}

async fn cached_larm_asr(
    providers: &crate::ModelProvidersSettings,
    audit: Option<&ConversationAudit>,
) -> Result<Arc<saaa_larm_session::Session>, String> {
    #[cfg(feature = "quality-eval-harness")]
    if let Ok(session) = crate::quality_eval::SESSION.try_with(Arc::clone) {
        return Ok(session);
    }
    let route = format!(
        "{}|{}",
        providers.harness.address,
        crate::providers::larm_resources::profile::label(
            &crate::providers::larm_resources::profile::preference(
                providers.harness.larm_profile.as_deref(),
            ),
        )
    );
    let mut cache = asr_session().lock().await;
    if let Some(cached) = cache.as_ref() {
        if cached.route == route {
            if let Some(audit) = audit {
                audit.event(
                    "provider",
                    "conversation-larm-cache",
                    "decision",
                    None,
                    json!({"result": "hit"}),
                );
            }
            return Ok(Arc::clone(&cached.session));
        }
    }
    if let Some(audit) = audit {
        audit.event(
            "provider",
            "conversation-larm-cache",
            "decision",
            None,
            json!({
                "result": if cache.is_some() { "replace" } else { "miss" },
            }),
        );
    }
    if let Some(previous) = cache.take() {
        if Arc::strong_count(&previous.session) == 1 {
            previous
                .session
                .close()
                .await
                .map_err(|_| "以前のLARM ASR接続を解放できませんでした。".to_string())?;
        }
    }
    let session = connect_larm(providers).await?;
    *cache = Some(CachedAsrSession {
        route,
        session: Arc::clone(&session),
    });
    Ok(session)
}

#[tauri::command]
pub(crate) async fn submit_conversation_text(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    input: SubmitInput,
    on_stage: tauri::ipc::Channel<ConversationStageEvent>,
) -> Result<SubmitResult, String> {
    let _ = input.source;
    queue_runtime::enqueue_text(&state, input.input_id.clone(), input.text)?;
    state.conversation_queue_wake.notify_waiters();
    let _ = app.emit("conversation-queue-updated", ());
    report_stage(&on_stage, "ornith");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(180);
    loop {
        let outcome =
            state.sqlite_readers.read(|connection| {
                let answer = connection.query_row(
                "SELECT content FROM conversation_messages WHERE id=?1 AND conversation_id=?2",
                params![format!("reply_{}", input.input_id), PRIMARY_CONVERSATION_ID],
                |row| row.get::<_, String>(0),
            ).optional().map_err(database_error)?;
                let status = connection
                    .query_row(
                        "SELECT status,error_message FROM runtime_runs WHERE id=?1",
                        [format!("run_{}", input.input_id)],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                    )
                    .optional()
                    .map_err(database_error)?;
                Ok((answer, status))
            })?;
        if let Some(content) = outcome.0 {
            return Ok(SubmitResult {
                content,
                model: "Ornith 1.5".into(),
                provider_label: "LARM llm".into(),
            });
        }
        if let Some((status, error)) = outcome.1 {
            if status == "failed" || status == "cancelled" {
                return Err(error.unwrap_or_else(|| "会話の処理は終了しました。".into()));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("会話の処理が時間内に完了しませんでした。".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

fn fit_role_history(
    instruction: &str,
    recent: &[(String, String)],
    text: &str,
    capacity: usize,
) -> Result<Vec<(String, String)>, String> {
    let mut fitted = recent.to_vec();
    loop {
        let mut messages = vec![json!({"role":"system","content":instruction})];
        messages.extend(
            fitted
                .iter()
                .map(|(role, content)| json!({"role":role,"content":content})),
        );
        messages.push(json!({"role":"user","content":text}));
        let bytes = serde_json::to_vec(&messages)
            .map_err(|error| error.to_string())?
            .len();
        if bytes <= capacity {
            return Ok(fitted);
        }
        let removable = fitted.iter().position(|(_, content)| {
            !content.starts_with("[ORNITH_RESULT") && !content.starts_with("[TOOL_RESULT")
        });
        let Some(index) = removable else {
            return Err(
                "Required context does not fit this provider. Narrow the task scope.".into(),
            );
        };
        fitted.remove(index);
    }
}

#[allow(clippy::too_many_arguments)]
async fn complete_larm_role_with_events(
    session: &Arc<saaa_larm_session::Session>,
    role: &str,
    recent: &[context_compiler::ContextEntry],
    text: &str,
    timeout_ms: u64,
    audit: &ConversationAudit,
    context_step: &context_compiler::ContextStep<'_>,
    metrics: &context_metrics::RequestMetrics,
    on_delta: Option<&dyn crate::runtime::event_hub::RuntimeEventSender>,
    cancellation: Option<Arc<RunCancellation>>,
) -> Result<(String, String), String> {
    let started = Instant::now();
    audit.event(
        "provider",
        "conversation-role-request",
        "request",
        None,
        json!({
            "role": role, "historyMessages": recent.len(), "timeoutMs": timeout_ms,
        }),
    );
    let result: Result<(String, String), String> = async {
        let lease = session.acquire(role).await.map_err(str::to_string)?;
        let provider = lease.provider();
        audit.event(
            "provider",
            "conversation-role-acquired",
            "start",
            Some("success"),
            json!({
                "role": role, "model": provider.model, "protocol": provider.protocol,
            }),
        );
        if provider.protocol != "openai.chat-completions.v1" {
            return Err(format!("{role}の会話プロトコルに対応していません。"));
        }
        let request_timeout_ms = timeout_ms.min(120_000);
        let budget = lease
            .request_budget(std::time::Duration::from_millis(request_timeout_ms))
            .map_err(str::to_string)?;
        let advertised = provider
            .context_window
            .ok_or("LARMのコンテキスト上限がありません。")?;
        // Bytes are a conservative upper bound for input tokens across the supported UTF-8
        // content. Keep the existing provider budget as an additional local ceiling.
        let mut input_budget =
            crate::runtime::context::broker::ProviderInputBudget::openai_compatible();
        if context_step.mode == context_compiler::PrefixMode::Stable {
            // Offered definitions are already included in the exact message array.
            input_budget = input_budget.with_tool_schema_reserve_bytes(0);
        }
        let capacity = input_budget
            .usable_context_bytes()
            .min((advertised.max_input_tokens() as usize).saturating_sub(2_048));
        // The input budget already reserves this output space. Do not shrink it for
        // tool follow-ups: a concise-answer instruction is not a token limit.
        let max_output_tokens = advertised.output_reserve_tokens.min(u32::MAX as u64) as u32;
        let metrics = metrics.for_provider(provider.base_url.as_str());
        let compiled = context_step
            .compile(recent, text, capacity)
            .inspect_err(|_| {
                metrics.invalidated();
            })?;
        metrics.compiled(compiled.omitted);
        let authorization = zeroize::Zeroizing::new(format!("Bearer {}", provider.token()));
        let options = saaa_larm_session::http_api::LlmOptions {
            tools: false,
            thinking: saaa_larm_session::http_api::Thinking::Auto,
            ..Default::default()
        };
        audit.event(
            "provider",
            "conversation-role-send",
            "start",
            None,
            json!({
                "role": role, "model": provider.model, "budgetMs": budget.as_millis() as u64,
                "thinking": "auto",
                "maxOutputTokens": max_output_tokens,
            }),
        );
        let content = complete_http_with_instruction(
            provider.base_url.as_str(),
            Some(authorization.as_str()),
            &provider.model,
            text,
            budget.as_millis() as u64,
            max_output_tokens,
            Some(&options),
            true,
            Some(&compiled.instruction),
            &compiled.recent,
            Some((audit, role)),
            on_delta,
            cancellation,
            Some(&metrics),
        )
        .await?;
        audit.text("provider", "conversation-ornith-output", &content);
        if role == "llm" && (content.contains("<think>") || content.contains("</think>")) {
            return Err("ornithの内部思考が回答本文に混入しました。".into());
        }
        Ok((content, provider.model.clone()))
    }
    .await;
    if result.as_ref().err().is_some_and(|error| {
        error
            == crate::providers::stream::ProviderFailureKind::Authentication
                .public_message()
                .as_str()
            || matches!(
                error.as_str(),
                "larm_authentication_failed"
                    | "larm_session_closed"
                    | "larm_session_unavailable"
                    | "larm_expired"
                    | "larm_provider_terminal"
                    | "larm_connection_idle_released"
            )
    }) {
        // Evict only this rejected session; an overlapping ASR reconnect may
        // already have installed a new one. Existing leases finish normally.
        let mut cache = asr_session().lock().await;
        if cache
            .as_ref()
            .is_some_and(|cached| Arc::ptr_eq(&cached.session, session))
        {
            cache.take();
        }
    }
    match &result {
        Ok((content, model)) => audit.event(
            "provider",
            "conversation-role-result",
            "terminal",
            Some("success"),
            json!({
                "role": role, "model": model, "textBytes": content.len(),
                "elapsedMs": started.elapsed().as_millis() as u64,
            }),
        ),
        Err(error) => audit.event(
            "provider",
            "conversation-role-result",
            "error",
            Some("failure"),
            json!({
                "role": role, "error": error,
                "elapsedMs": started.elapsed().as_millis() as u64,
            }),
        ),
    }
    result
}

async fn connect_larm(
    providers: &crate::ModelProvidersSettings,
) -> Result<Arc<saaa_larm_session::Session>, String> {
    #[cfg(feature = "conversation-queue-e2e")]
    let fixture_token = crate::conversation_queue_e2e::credential();
    #[cfg(not(feature = "conversation-queue-e2e"))]
    let fixture_token: Option<String> = None;
    let token = if let Some(token) = fixture_token {
        token
    } else {
        crate::providers::dynamic_lan::credential::load()
            .map_err(|error| error.code().to_string())?
            .token()
            .to_string()
    };
    let preference = crate::providers::larm_resources::profile::preference(
        providers.harness.larm_profile.as_deref(),
    );
    let (_stop, receiver) = tokio::sync::watch::channel(false);
    let connection =
        saaa_larm_session::Session::connect_with_profile_credential_key_phase_and_providers(
            &providers.harness.address,
            preference,
            token,
            format!("saaa-conversation-check-{}", uuid::Uuid::new_v4().simple()),
            receiver,
            None,
            Some(vec!["tts", "asr", "llm", "embedding"]),
        )
        .await;
    let session = match connection {
        Ok(session) => session,
        Err(error) => {
            if let Some(cleanup) = &error.cleanup {
                let _ = cleanup.close().await;
            }
            let detail = match error.code {
                "larm_provider_not_claimable" => "LARMは接続準備完了を返しましたが、プロバイダーの認証情報を取得できない状態です。",
                "larm_provider_not_ready" => "LARMは接続準備完了を返しましたが、準備が完了していないプロバイダーがあります。",
                _ => "LARMへの接続に失敗しました。",
            };
            return Err(format!("{detail} ({error})"));
        }
    };
    Ok(session)
}

pub(crate) async fn complete_http(
    endpoint: &str,
    authorization: Option<&str>,
    model: &str,
    text: &str,
    timeout_ms: u64,
    configured_options: Option<&saaa_larm_session::http_api::LlmOptions>,
    no_proxy: bool,
) -> Result<String, String> {
    complete_http_with_instruction(
        endpoint,
        authorization,
        model,
        text,
        timeout_ms,
        512,
        configured_options,
        no_proxy,
        None,
        &[],
        None,
        None,
        None,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn complete_http_with_instruction(
    endpoint: &str,
    authorization: Option<&str>,
    model: &str,
    text: &str,
    timeout_ms: u64,
    max_output_tokens: u32,
    configured_options: Option<&saaa_larm_session::http_api::LlmOptions>,
    no_proxy: bool,
    instruction: Option<&str>,
    recent: &[(String, String)],
    audit: Option<(&ConversationAudit, &str)>,
    on_delta: Option<&dyn crate::runtime::event_hub::RuntimeEventSender>,
    cancellation: Option<Arc<RunCancellation>>,
    observation: Option<&dyn crate::providers::chat_completions::observation::ObservationSink>,
) -> Result<String, String> {
    let input = StartTurnInput {
        run_id: format!("check_{}", uuid::Uuid::new_v4().simple()),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: text.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let user = crate::ipc_contract::ConversationMessage {
        parts: None,
        id: input.run_id.clone(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: text.into(),
        created_at: String::new(),
    };
    let mut history = Vec::with_capacity(2);
    if let Some(instruction) = instruction {
        history.push(crate::ipc_contract::ConversationMessage {
            parts: None,
            id: format!("{}_system", input.run_id),
            conversation_id: input.conversation_id.clone(),
            role: "system".into(),
            content: instruction.into(),
            created_at: String::new(),
        });
    }
    for (index, (role, content)) in recent.iter().enumerate() {
        history.push(crate::ipc_contract::ConversationMessage {
            parts: None,
            id: format!("{}_prior_{index}", input.run_id),
            conversation_id: input.conversation_id.clone(),
            role: role.clone(),
            content: content.clone(),
            created_at: String::new(),
        });
    }
    history.push(user);
    let sink = tauri::ipc::Channel::new(|_| Ok(()));
    let options = configured_options.cloned().unwrap_or_default();
    let make_context = || crate::providers::stream::ModelStreamContext {
        reasoning_effort: "provider-default",
        max_output_tokens,
        input: &input,
        on_event: on_delta.unwrap_or(&sink),
        cancellation: cancellation
            .clone()
            .unwrap_or_else(|| Arc::new(RunCancellation::default())),
        context_health: "green",
        context_sources: &[],
        context_omissions: &[],
        output_persistence: None,
    };
    let mode = if on_delta.is_some() {
        crate::providers::chat_completions::RequestMode::Stream
    } else {
        crate::providers::chat_completions::RequestMode::JsonProbe
    };
    let mut result = crate::providers::chat_completions::run_observed(
        endpoint,
        authorization,
        model,
        &history,
        timeout_ms.min(120_000),
        make_context(),
        mode,
        &options,
        no_proxy,
        observation,
    )
    .await;
    // Some configured Chat Completions servers reject SSE. Preserve the existing
    // complete-response path only if no streamed output has reached the speaker.
    if on_delta.is_some()
        && matches!(
            result,
            Err(crate::providers::stream::ProviderAttemptError::Failed {
                kind: crate::providers::stream::ProviderFailureKind::Contract
                    | crate::providers::stream::ProviderFailureKind::Protocol,
                output_started: false,
                ..
            })
        )
    {
        result = crate::providers::chat_completions::run_observed(
            endpoint,
            authorization,
            model,
            &history,
            timeout_ms.min(120_000),
            make_context(),
            crate::providers::chat_completions::RequestMode::JsonProbe,
            &options,
            no_proxy,
            observation,
        )
        .await;
        if let (Ok(content), Some(on_delta)) = (&result, on_delta) {
            on_delta
                .send(crate::ipc_contract::RuntimeEvent::Delta {
                    run_id: input.run_id.clone(),
                    text: content.clone(),
                })
                .map_err(|_| "音声用の回答を受け渡せませんでした。".to_string())?;
        }
    }
    result.map_err(|error| match error {
        crate::providers::stream::ProviderAttemptError::Failed {
            kind,
            output_started,
            detail,
        } => {
            if let Some((audit, role)) = audit {
                audit.event(
                    "provider",
                    "conversation-provider-failure",
                    "error",
                    Some("failure"),
                    json!({
                        "role": role, "kind": kind.as_str(), "outputStarted": output_started,
                        "detail": detail,
                    }),
                );
            }
            kind.public_message().as_str().to_string()
        }
        crate::providers::stream::ProviderAttemptError::Cancelled { output_started } => {
            if let Some((audit, role)) = audit {
                audit.event(
                    "provider",
                    "conversation-provider-failure",
                    "error",
                    Some("cancelled"),
                    json!({
                        "role": role, "kind": "cancelled", "outputStarted": output_started,
                    }),
                );
            }
            "Providerの処理が中断されました。".into()
        }
    })
}

#[cfg(test)]
mod jarvis_tests {
    use super::{fit_role_history, speech_text_for_answer};

    #[test]
    fn replay_reads_answer_without_source_links() {
        let saved = "今日は晴れです。\n\n<!-- saaa:source-links -->\n[出典1: example.com](https://example.com/weather)\n";
        assert_eq!(speech_text_for_answer(saved), "今日は晴れです。");
    }

    #[test]
    fn provider_window_discards_optional_history_but_keeps_current_input() {
        let history = vec![
            ("user".into(), "古い会話".repeat(100)),
            ("user".into(), "WorldModelの資料".into()),
        ];
        let fitted = fit_role_history("固定ポリシー", &history, "今の依頼", 150).unwrap();
        assert_eq!(fitted.len(), 1);
        assert!(fitted[0].1.contains("WorldModel"));
        assert!(fit_role_history("固定ポリシー", &[], &"大".repeat(200), 150).is_err());
        let required = vec![(
            "user".into(),
            "[ORNITH_RESULT]".to_string() + &"根拠".repeat(100),
        )];
        assert!(fit_role_history("固定ポリシー", &required, "今の依頼", 150).is_err());
    }
}
