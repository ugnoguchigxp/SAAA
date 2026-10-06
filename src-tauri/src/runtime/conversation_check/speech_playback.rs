//! The same playback path for purpose-selected streaming and progress speech.
use super::super::speech_expression::ChunkCue;
use super::*;
#[allow(clippy::too_many_arguments)]
pub(in crate::runtime::conversation_check) async fn play_chunk<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    input_id: &str,
    state: &AppState,
    resolved: &crate::providers::service_registry::ResolvedRoute,
    providers: &crate::ModelProvidersSettings,
    route: &crate::VoiceRouteSettings,
    text: &str,
    dictionary: &crate::tts_dictionary::CompiledDictionary,
    audit: &ConversationAudit,
    cancellation: Arc<RunCancellation>,
    continuous: &mut Option<crate::voice::local_audio_output::ContinuousPlayback>,
) -> Result<(), String> {
    state
        .sqlite_readers
        .read(|db| direct_route::validate_route(db, resolved))?;
    if cancellation.is_cancelled() {
        return Err("Speech cancelled".into());
    }
    let spoken = dictionary.apply(text);
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
    let harness = &providers.harness;
    let budget = timeout.min(2000);
    let (expression, cue) =
        ChunkCue::prepare(app, input_id, state, harness, text, &cancellation, budget).await?;
    let on_chunk = cue.on_audio_ready(cancellation.clone());
    state
        .sqlite_readers
        .read(|db| direct_route::validate_route(db, resolved))?;
    if route.source == "harness" {
        let session = note_larm_connect(state, cached_larm_asr(providers, Some(audit)).await)?;
        state
            .sqlite_readers
            .read(|db| direct_route::validate_route(db, resolved))?;
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
            expression,
            output,
            text,
            timeout,
            cancellation,
            on_chunk,
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
            let expressed =
                crate::voice::cloud_tts::speech_directive::apply_expression(provider, expression);
            crate::voice::http_audio::play_with_situation(
                &expressed,
                text,
                timeout,
                cancellation,
                output,
                on_chunk,
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
            on_chunk();
            player.play_wav_file(&path, &cancellation).await
        }
        _ => Err("設定済みの音声出力ルートはTTS Providerではありません。".into()),
    }
}
