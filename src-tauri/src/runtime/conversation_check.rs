//! Conversation-screen turn path: finalized ASR text, Qwen handoff, ornith answer and TTS.
//! Continuous partial-ASR routing and the durable work queue remain separate runtime work.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::time::Instant;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    database_error, now_iso, persistence, validate_identifier, AppState, ModelProviderSettings,
    RunCancellation, StartTurnInput, PRIMARY_CONVERSATION_ID,
};

static BUSY: AtomicBool = AtomicBool::new(false);
static ASR_SESSION: OnceLock<tokio::sync::Mutex<Option<CachedAsrSession>>> = OnceLock::new();
static SPEECH_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

struct CachedAsrSession {
    route: String,
    session: Arc<saaa_larm_session::Session>,
}

fn asr_session() -> &'static tokio::sync::Mutex<Option<CachedAsrSession>> {
    ASR_SESSION.get_or_init(|| tokio::sync::Mutex::new(None))
}

struct BusyGuard;
impl BusyGuard {
    fn acquire() -> Result<Self, String> {
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
            .map(|_| Self)
            .map_err(|_| "会話の応答を処理中です。".to_string())
    }
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
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
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
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
#[serde(deny_unknown_fields)]
struct QwenDecision {
    route: String,
    reply: Option<String>,
}

fn parse_qwen_decision(raw: &str) -> Result<QwenDecision, String> {
    let decision: QwenDecision = serde_json::from_str(raw.trim())
        .map_err(|_| "Qwenの振り分け結果を読み取れませんでした。".to_string())?;
    match decision.route.as_str() {
        "quick"
            if decision
                .reply
                .as_deref()
                .is_some_and(|reply| !reply.trim().is_empty() && reply.len() <= 1000) =>
        {
            Ok(decision)
        }
        "think" if decision.reply.is_none() => Ok(decision),
        _ => Err("Qwenの振り分け結果が契約に合いません。".into()),
    }
}

fn is_standalone_greeting(text: &str) -> bool {
    let normalized = text
        .trim()
        .trim_end_matches(['。', '！', '!', '？', '?', '、', '.', ' '])
        .to_lowercase();
    matches!(
        normalized.as_str(),
        "こんにちは"
            | "こんにちわ"
            | "こんばんは"
            | "おはよう"
            | "おはようございます"
            | "もしもし"
            | "hello"
            | "hi"
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
    let result = speak_conversation_answer_inner(&state, &input_id, &audit).await;
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

async fn speak_conversation_answer_inner(
    state: &AppState,
    input_id: &str,
    audit: &ConversationAudit,
) -> Result<(), String> {
    let _speech = SPEECH_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let answer_id = format!("reply_{input_id}");
    let (content, providers, route) = state.sqlite_readers.read(|connection| {
        let content: String = connection.query_row(
            "SELECT content FROM conversation_messages WHERE id=?1 AND conversation_id=?2 AND role='assistant'",
            params![answer_id, PRIMARY_CONVERSATION_ID], |row| row.get(0),
        ).map_err(database_error)?;
        Ok((content, persistence::load_model_providers(connection)?,
            persistence::load_routing_settings(connection)?.voice_speak))
    })?;
    if content.trim().is_empty() || content.len() > 8192 {
        return Err("読み上げる回答がありません。".into());
    }
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
        let result = crate::voice::http_audio::play_larm_with_situation(
            &session,
            PRIMARY_CONVERSATION_ID,
            providers.harness.tts_voice.as_deref(),
            Some(&providers.harness),
            crate::voice::cloud_tts::speech_directive::SpeechExpression::Natural,
            output,
            &content,
            route.timeout_ms.min(120_000),
            cancellation,
            move || {
                playback_audit.event("tts", "conversation-tts-playback", "start", None, json!({}))
            },
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
            crate::voice::http_audio::play_with_situation(
                provider,
                &content,
                route.timeout_ms.min(120_000),
                cancellation,
                output,
                move || {
                    playback_audit.event(
                        "tts",
                        "conversation-tts-playback",
                        "start",
                        None,
                        json!({}),
                    )
                },
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
            let directory =
                tempfile::tempdir().map_err(|_| "TTS一時領域を作成できませんでした。")?;
            let path = crate::voice::system_tts::render_tts_artifact(
                content,
                provider.voice.clone(),
                directory.path().to_path_buf(),
                cancellation,
            )
            .await?;
            audit.event(
                "tts",
                "conversation-tts-render",
                "terminal",
                Some("success"),
                json!({}),
            );
            let playback_audit = audit.clone();
            tokio::task::spawn_blocking(move || {
                let mut child = crate::voice::cloud_tts::spawn_audio_player(&path)?;
                playback_audit.event("tts", "conversation-tts-playback", "start", None, json!({}));
                let status = child
                    .wait()
                    .map_err(|_| "TTS再生を確認できませんでした。".to_string())?;
                if status.success() {
                    Ok(())
                } else {
                    Err("TTS再生に失敗しました。".into())
                }
            })
            .await
            .map_err(|_| "TTS再生が中断されました。".to_string())?
        }
        _ => Err("設定済みの音声出力ルートはTTS Providerではありません。".into()),
    }
}

async fn cached_larm_asr(
    providers: &crate::ModelProvidersSettings,
    audit: Option<&ConversationAudit>,
) -> Result<Arc<saaa_larm_session::Session>, String> {
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
    input: SubmitInput,
    on_stage: tauri::ipc::Channel<ConversationStageEvent>,
) -> Result<SubmitResult, String> {
    validate_identifier(&input.input_id, "input id")?;
    let audit = ConversationAudit::new(state.sqlite_writer.clone(), input.input_id.clone());
    let started = Instant::now();
    audit.event(
        "conversation",
        "conversation-submit-request",
        "request",
        None,
        json!({
            "source": if matches!(input.source, CheckSource::Larm) { "larm" } else { "configured" },
            "textBytes": input.text.len(),
        }),
    );
    if input.text.len() <= 4_096 {
        audit.text("conversation", "conversation-input-text", &input.text);
    } else {
        audit.event(
            "conversation",
            "conversation-input-rejected",
            "decision",
            Some("failure"),
            json!({
                "reason": "too-large", "textBytes": input.text.len(),
            }),
        );
    }
    let result = submit_conversation_text_inner(&state, input, &on_stage, &audit).await;
    match &result {
        Ok(value) => audit.event(
            "conversation",
            "conversation-submit-result",
            "terminal",
            Some("success"),
            json!({
                "model": value.model, "provider": value.provider_label,
                "elapsedMs": started.elapsed().as_millis() as u64,
            }),
        ),
        Err(error) => audit.event(
            "conversation",
            "conversation-submit-result",
            "error",
            Some("failure"),
            json!({
                "error": error, "elapsedMs": started.elapsed().as_millis() as u64,
            }),
        ),
    }
    result
}

async fn submit_conversation_text_inner(
    state: &AppState,
    input: SubmitInput,
    on_stage: &tauri::ipc::Channel<ConversationStageEvent>,
    audit: &ConversationAudit,
) -> Result<SubmitResult, String> {
    validate_identifier(&input.input_id, "input id")?;
    if input.text.trim().is_empty() || input.text.len() > 4096 {
        return Err("入力は1〜4096バイトにしてください。".into());
    }
    let _busy = BusyGuard::acquire()?;
    let user_id = format!("check_{}", input.input_id);
    let answer_id = format!("reply_{}", input.input_id);
    let saved = state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT u.content, a.content FROM conversation_messages u \
             JOIN conversation_messages a ON a.id=?2 \
             WHERE u.id=?1 AND u.conversation_id=?3 AND a.conversation_id=?3",
                params![user_id, answer_id, PRIMARY_CONVERSATION_ID],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(database_error)
    })?;
    if let Some((original, content)) = saved {
        if original != input.text {
            return Err("同じ入力IDで異なる本文は送信できません。".into());
        }
        audit.event(
            "conversation",
            "conversation-submit-deduplicated",
            "decision",
            Some("success"),
            json!({}),
        );
        return Ok(SubmitResult {
            content,
            model: "保存済み".into(),
            provider_label: "会話記録".into(),
        });
    }
    let (providers, route) = state.sqlite_readers.read(|connection| {
        Ok((
            persistence::load_model_providers(connection)?,
            persistence::load_routing_settings(connection)?.conversation_respond,
        ))
    })?;
    let (content, model, provider_label) =
        if matches!(input.source, CheckSource::Larm) || route.source == "harness" {
            audit.event(
                "conversation",
                "conversation-response-route",
                "decision",
                None,
                json!({
                    "source": "larm", "timeoutMs": route.timeout_ms,
                }),
            );
            let mut recent =
                state.sqlite_readers.read(|connection| {
                    let mut statement = connection.prepare(
                "SELECT role, content FROM conversation_messages WHERE conversation_id=?1 \
                 AND role IN ('user','assistant') ORDER BY created_at DESC, rowid DESC LIMIT 8",
            ).map_err(database_error)?;
                    let rows = statement
                        .query_map(params![PRIMARY_CONVERSATION_ID], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })
                        .map_err(database_error)?;
                    rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
                })?;
            recent.reverse();
            complete_larm_jarvis(
                &providers,
                &recent,
                &input.text,
                route.timeout_ms,
                on_stage,
                audit,
            )
            .await?
        } else {
            audit.event(
                "conversation",
                "conversation-response-route",
                "decision",
                None,
                json!({
                    "source": "configured", "providerId": route.primary_provider_id,
                    "timeoutMs": route.timeout_ms,
                }),
            );
            let provider = providers
                .providers
                .iter()
                .find(|candidate| {
                    route.primary_provider_id.as_deref() == Some(candidate.id())
                        && candidate.enabled()
                })
                .ok_or("設定済みの会話用Providerが見つかりません。")?;
            match provider {
                ModelProviderSettings::OpenAiCompatible(provider) => {
                    let key = crate::providers::openai_compatible::provider_api_key(provider)?;
                    if provider.authentication == "api-key" && key.is_none() {
                        return Err("会話用ProviderのAPIキーがありません。".into());
                    }
                    let authorization = key
                        .as_deref()
                        .map(|key| zeroize::Zeroizing::new(format!("Bearer {key}")));
                    audit.event(
                        "provider",
                        "conversation-role-send",
                        "start",
                        None,
                        json!({
                            "role": "configured", "model": provider.model,
                        }),
                    );
                    let content = complete_http_with_instruction(
                        &provider.endpoint,
                        authorization.as_deref().map(String::as_str),
                        &provider.model,
                        &input.text,
                        route.timeout_ms,
                        provider.request_options.as_ref(),
                        false,
                        None,
                        &[],
                        Some((audit, "configured")),
                    )
                    .await?;
                    (content, provider.model.clone(), provider.label.clone())
                }
                _ => return Err("この会話用Provider形式は最小確認画面では未対応です。".into()),
            }
        };
    audit.text("conversation", "conversation-output-text", &content);
    if content.trim().is_empty() || content.len() > 8192 {
        return Err("Providerの回答が空か、上限を超えました。".into());
    }
    audit.event(
        "conversation",
        "conversation-reply-persist",
        "start",
        None,
        json!({
            "model": model, "provider": provider_label, "textBytes": content.len(),
        }),
    );
    state.sqlite_writer.write(|connection| {
        let transaction = connection.transaction().map_err(database_error)?;
        let now = now_iso();
        transaction
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) \
             VALUES(?1,?2,'user',?3,?4)",
                params![user_id, PRIMARY_CONVERSATION_ID, input.text, now],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) \
             VALUES(?1,?2,'assistant',?3,?4)",
                params![answer_id, PRIMARY_CONVERSATION_ID, content, now],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at=?1 WHERE id=?2",
                params![now, PRIMARY_CONVERSATION_ID],
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO audit_events(id,occurred_at,component,event_name,phase,outcome,correlation_id,conversation_id,attributes_json) \
                 VALUES(?1,?2,'conversation','reply-route','terminal','success',?3,?4,?5)",
                params![
                    format!("audit_{}", uuid::Uuid::new_v4().simple()),
                    now,
                    input.input_id,
                    PRIMARY_CONVERSATION_ID,
                    serde_json::json!({
                        "provider": provider_label,
                        "model": model,
                        "qwenDecision": if provider_label.contains("ornith") {
                            "think"
                        } else if provider_label.contains("Qwen") {
                            "quick"
                        } else {
                            "not-applicable"
                        },
                    }).to_string(),
                ],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)
    })?;
    audit.event(
        "conversation",
        "conversation-reply-persist",
        "terminal",
        Some("success"),
        json!({
            "model": model, "provider": provider_label,
        }),
    );
    Ok(SubmitResult {
        content,
        model,
        provider_label,
    })
}

async fn complete_larm_jarvis(
    providers: &crate::ModelProvidersSettings,
    recent: &[(String, String)],
    text: &str,
    timeout_ms: u64,
    on_stage: &tauri::ipc::Channel<ConversationStageEvent>,
    audit: &ConversationAudit,
) -> Result<(String, String, String), String> {
    let started = Instant::now();
    audit.event(
        "provider",
        "conversation-larm-connect",
        "start",
        None,
        json!({
            "profile": providers.harness.larm_profile,
        }),
    );
    let session = match cached_larm_asr(providers, Some(audit)).await {
        Ok(session) => {
            audit.event(
                "provider",
                "conversation-larm-connect",
                "terminal",
                Some("success"),
                json!({
                    "elapsedMs": started.elapsed().as_millis() as u64,
                }),
            );
            session
        }
        Err(error) => {
            audit.event(
                "provider",
                "conversation-larm-connect",
                "error",
                Some("failure"),
                json!({
                    "error": error, "elapsedMs": started.elapsed().as_millis() as u64,
                }),
            );
            return Err(error);
        }
    };
    let answer = async {
        report_stage(on_stage, "qwen");
        if is_standalone_greeting(text) {
            let (content, model) = complete_larm_role(
                &session, "backchannel", recent, text, timeout_ms, audit,
                "あなたは会話の入口です。挨拶には自然な日本語で短く応答してください。JSONや内部思考を出力しないでください。依頼や事実を作らないでください。",
            ).await?;
            audit.event("conversation", "conversation-qwen-decision", "decision", Some("success"), json!({
                "route": "quick", "reason": "standalone-greeting",
            }));
            return Ok((content, model, "LARM Qwen backchannel".into()));
        }
        let (control, qwen_model) = complete_larm_role(
            &session, "backchannel", recent, text, timeout_ms, audit,
            "あなたは会話の入口です。必ずJSONオブジェクトだけを返す。形式は {\"route\":\"quick\",\"reply\":\"短い回答\"} または {\"route\":\"think\",\"reply\":null}。挨拶、お礼、現在の発言だけで確実に答えられる簡単な質問はquick。最新情報、過去の会話、調査、計画、操作、曖昧な内容はthink。事実や進捗を推測しない。",
        ).await?;
        // A malformed routing response must never become a quick answer.
        // Send it to the thinking role, which can still answer the request.
        let decision = match parse_qwen_decision(&control) {
            Ok(decision) => {
                audit.event("conversation", "conversation-qwen-decision", "decision", Some("success"), json!({
                    "route": decision.route, "reason": "model-decision",
                }));
                decision
            }
            Err(error) => {
                audit.event("conversation", "conversation-qwen-decision", "decision", Some("degraded"), json!({
                    "route": "think", "reason": "invalid-qwen-output", "parseError": error,
                }));
                QwenDecision { route: "think".into(), reply: None }
            }
        };
        if decision.route == "quick" {
            return Ok((decision.reply.unwrap_or_default(), qwen_model, "LARM Qwen backchannel".into()));
        }
        report_stage(on_stage, "ornith");
        let (content, model) = complete_larm_role(
            &session, "llm", recent, text, timeout_ms, audit,
            "あなたはSAAAの思考担当です。ユーザーの依頼に日本語で正確に答える。利用できない外部情報や操作結果を作らず、必要なら制限を明示する。内部推論は表示せず、ユーザー向けの回答本文だけを返す。",
        ).await?;
        Ok::<_, String>((content, model, "LARM ornith llm".into()))
    }
    .await;
    answer
}

async fn complete_larm_role(
    session: &Arc<saaa_larm_session::Session>,
    role: &str,
    recent: &[(String, String)],
    text: &str,
    timeout_ms: u64,
    audit: &ConversationAudit,
    instruction: &str,
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
        let budget = lease
            .request_budget(std::time::Duration::from_millis(timeout_ms.min(120_000)))
            .map_err(str::to_string)?;
        let authorization = zeroize::Zeroizing::new(format!("Bearer {}", provider.token()));
        let options = saaa_larm_session::http_api::LlmOptions {
            thinking: if role == "backchannel" {
                saaa_larm_session::http_api::Thinking::Disabled
            } else {
                saaa_larm_session::http_api::Thinking::Auto
            },
            ..Default::default()
        };
        audit.event(
            "provider",
            "conversation-role-send",
            "start",
            None,
            json!({
                "role": role, "model": provider.model, "budgetMs": budget.as_millis() as u64,
                "thinking": if role == "backchannel" { "disabled" } else { "auto" },
            }),
        );
        let content = complete_http_with_instruction(
            provider.base_url.as_str(),
            Some(authorization.as_str()),
            &provider.model,
            text,
            budget.as_millis() as u64,
            Some(&options),
            true,
            Some(instruction),
            recent,
            Some((audit, role)),
        )
        .await?;
        audit.text(
            "provider",
            if role == "backchannel" {
                "conversation-qwen-output"
            } else {
                "conversation-ornith-output"
            },
            &content,
        );
        if role == "llm" && (content.contains("<think>") || content.contains("</think>")) {
            return Err("ornithの内部思考が回答本文に混入しました。".into());
        }
        Ok((content, provider.model.clone()))
    }
    .await;
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
    let credential = crate::providers::dynamic_lan::credential::load()
        .map_err(|error| error.code().to_string())?;
    let preference = crate::providers::larm_resources::profile::preference(
        providers.harness.larm_profile.as_deref(),
    );
    let (_stop, receiver) = tokio::sync::watch::channel(false);
    let connection = saaa_larm_session::Session::connect_with_profile_credential_and_key(
        &providers.harness.address,
        preference,
        credential.token().to_string(),
        format!("saaa-conversation-check-{}", uuid::Uuid::new_v4().simple()),
        receiver,
    )
    .await;
    let session = match connection {
        Ok(session) => session,
        Err(error) => {
            if let Some(cleanup) = &error.cleanup {
                let _ = cleanup.close().await;
            }
            return Err(format!("LARMへの接続に失敗しました: {error}"));
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
        configured_options,
        no_proxy,
        None,
        &[],
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
    configured_options: Option<&saaa_larm_session::http_api::LlmOptions>,
    no_proxy: bool,
    instruction: Option<&str>,
    recent: &[(String, String)],
    audit: Option<(&ConversationAudit, &str)>,
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
    crate::providers::chat_completions::run_with_proxy_policy(
        endpoint,
        authorization,
        model,
        &history,
        timeout_ms.min(120_000),
        crate::providers::stream::ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: if instruction.is_some() { 2_048 } else { 512 },
            input: &input,
            on_event: &sink,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
        crate::providers::chat_completions::RequestMode::JsonProbe,
        &options,
        no_proxy,
    )
    .await
    .map_err(|error| match error {
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
    use super::{is_standalone_greeting, parse_qwen_decision};

    #[test]
    fn only_standalone_greetings_use_qwen_directly() {
        assert!(is_standalone_greeting("こんにちは。"));
        assert!(is_standalone_greeting("こんにちわ！"));
        assert!(is_standalone_greeting("Hello!"));
        assert!(!is_standalone_greeting("こんにちは。株価を教えて"));
        assert!(!is_standalone_greeting("マイクロソフトの株価を教えて"));
    }

    #[test]
    fn quick_reply_requires_nonempty_bounded_body() {
        let quick = parse_qwen_decision(r#"{"route":"quick","reply":"こんにちは。"}"#).unwrap();
        assert_eq!(quick.reply.as_deref(), Some("こんにちは。"));
        assert!(parse_qwen_decision(r#"{"route":"quick","reply":""}"#).is_err());
        assert!(parse_qwen_decision(r#"{"route":"quick","reply":null}"#).is_err());
    }

    #[test]
    fn thought_handoff_cannot_publish_qwen_body() {
        assert!(parse_qwen_decision(r#"{"route":"think","reply":null}"#).is_ok());
        assert!(parse_qwen_decision(r#"{"route":"think","reply":"調査しました"}"#).is_err());
        assert!(parse_qwen_decision(r#"{"route":"quick","reply":"はい","extra":true}"#).is_err());
    }
}
