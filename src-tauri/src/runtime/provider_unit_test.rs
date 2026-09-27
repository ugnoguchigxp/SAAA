//! Interactive, read-only probes of one claimed LARM provider at a time.
use base64::Engine;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};

use crate::{persistence, AppState, RunCancellation};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UnitTestInput {
    capability: String,
    text: Option<String>,
    audio_upload_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnitTestResult {
    capability: String,
    model: String,
    output: String,
    latency_ms: u64,
    audio_base64: Option<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct UnitTestProgress {
    stage: String,
}

struct CachedTextSession {
    address: String,
    preference: saaa_larm_session::ProfilePreference,
    credential: zeroize::Zeroizing<String>,
    provider_scope: Option<&'static str>,
    session: Arc<saaa_larm_session::Session>,
    generation: u64,
}

static TEXT_SESSION: OnceLock<tokio::sync::Mutex<Option<CachedTextSession>>> = OnceLock::new();
static TEXT_SESSION_GENERATION: AtomicU64 = AtomicU64::new(0);

fn text_session() -> &'static tokio::sync::Mutex<Option<CachedTextSession>> {
    TEXT_SESSION.get_or_init(|| tokio::sync::Mutex::new(None))
}

#[tauri::command]
pub(crate) async fn run_provider_unit_test(
    state: tauri::State<'_, AppState>,
    input: UnitTestInput,
    on_progress: tauri::ipc::Channel<UnitTestProgress>,
) -> Result<UnitTestResult, String> {
    let capability = input.capability.as_str();
    if !matches!(
        capability,
        "asr" | "tts" | "backchannel" | "llm" | "embedding"
    ) {
        return Err("テスト対象のProviderが不正です。".into());
    }
    let text = input.text.as_deref().unwrap_or("").trim();
    if capability != "asr" && (text.is_empty() || text.len() > 4096) {
        return Err("入力は1〜4096バイトにしてください。".into());
    }
    let samples = if capability == "asr" {
        let id = input
            .audio_upload_id
            .as_deref()
            .ok_or("録音がありません。")?;
        let samples = state.audio_uploads.consume(id, "provider-unit-asr")?;
        if !(1_600..=16_000 * 120).contains(&samples.len()) {
            return Err("録音は0.1秒以上、2分以内にしてください。".into());
        }
        Some(samples)
    } else {
        None
    };
    let providers = state
        .sqlite_readers
        .read(persistence::load_model_providers)?;
    if providers.harness.address.trim().is_empty() {
        return Err("設定画面でLARMの接続先を保存してください。".into());
    }
    let credential = crate::providers::dynamic_lan::credential::load()
        .map_err(|error| error.code().to_string())?;
    run_with_settings(
        &providers,
        credential.token(),
        capability,
        text,
        samples.as_deref(),
        true,
        &|stage| {
            let _ = on_progress.send(UnitTestProgress {
                stage: stage.into(),
            });
        },
    )
    .await
}

async fn run_with_settings(
    providers: &crate::ModelProvidersSettings,
    credential: &str,
    capability: &str,
    text: &str,
    samples: Option<&[f32]>,
    reuse_text_session: bool,
    on_progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<UnitTestResult, String> {
    let started = std::time::Instant::now();
    let preference = crate::providers::larm_resources::profile::preference(
        providers.harness.larm_profile.as_deref(),
    );
    if capability == "tts" {
        on_progress("provider_request");
        let (model, output, audio_base64) =
            run_direct_tts(&providers.harness, credential, &preference, text).await?;
        return Ok(UnitTestResult {
            capability: capability.into(),
            model,
            output,
            latency_ms: started.elapsed().as_millis() as u64,
            audio_base64,
        });
    }
    if reuse_text_session && matches!(capability, "backchannel" | "llm" | "embedding") {
        return run_cached_text(
            providers,
            credential,
            preference,
            capability,
            text,
            started,
            on_progress,
        )
        .await;
    }
    let session = connect_session(
        providers,
        credential,
        preference,
        provider_scope(capability),
        on_progress,
    )
    .await?;
    on_progress("provider_request");
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        run_claimed(&session, capability, text, samples, &providers.harness),
    )
    .await
    .map_err(|_| "Providerの応答が時間内に完了しませんでした。".to_string())
    .and_then(|result| result);
    on_progress("releasing");
    let closed = session.close().await;
    if closed.is_err() {
        return Err("LARM接続の解放を確認できませんでした。".into());
    }
    let (model, output, audio_base64) =
        result.map_err(|error| crate::redact::redact_runtime_text(&error))?;
    Ok(UnitTestResult {
        capability: capability.into(),
        model,
        output,
        latency_ms: started.elapsed().as_millis() as u64,
        audio_base64,
    })
}

async fn connect_session(
    providers: &crate::ModelProvidersSettings,
    credential: &str,
    preference: saaa_larm_session::ProfilePreference,
    provider_scope: Option<&'static str>,
    on_progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<Arc<saaa_larm_session::Session>, String> {
    let (_stop, receiver) = tokio::sync::watch::channel(false);
    let (phase, mut changes) =
        tokio::sync::watch::channel(saaa_larm_session::ConnectionPhase::ModelPreparing);
    on_progress("model_preparing");
    // Connection startup owns its deadline and cleanup. A per-request 45s
    // timeout aborts valid cold starts before an ASR request can be made.
    let connecting =
        saaa_larm_session::Session::connect_with_profile_credential_key_phase_and_providers(
            &providers.harness.address,
            preference,
            credential.to_string(),
            format!("saaa-unit-test-{}", uuid::Uuid::new_v4().simple()),
            receiver,
            Some(phase),
            provider_scope.map(|name| vec![name]),
        );
    tokio::pin!(connecting);
    let mut observing = true;
    let connection = loop {
        tokio::select! {
            result = &mut connecting => break result,
            changed = changes.changed(), if observing => {
                observing = changed.is_ok();
                if observing {
                    on_progress(changes.borrow_and_update().as_str());
                }
            }
        }
    };
    match connection {
        Ok(session) => Ok(session),
        Err(error) => {
            if let Some(cleanup) = &error.cleanup {
                let _ = cleanup.close().await;
            }
            return Err(crate::redact::redact_runtime_text(&error.to_string()));
        }
    }
}

fn provider_scope(capability: &str) -> Option<&'static str> {
    match capability {
        "asr" => Some("asr"),
        "backchannel" => Some("backchannel"),
        "llm" => Some("llm"),
        "embedding" => Some("embedding"),
        _ => None,
    }
}

async fn run_cached_text(
    providers: &crate::ModelProvidersSettings,
    credential: &str,
    preference: saaa_larm_session::ProfilePreference,
    capability: &str,
    text: &str,
    started: std::time::Instant,
    on_progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<UnitTestResult, String> {
    let mut cached = text_session().lock().await;
    let scope = provider_scope(capability);
    let matches_settings = cached.as_ref().is_some_and(|entry| {
        entry.address == providers.harness.address
            && entry.preference == preference
            && entry.credential.as_str() == credential
            && entry.provider_scope == scope
    });
    if !matches_settings {
        if let Some(previous) = cached.take() {
            previous
                .session
                .close()
                .await
                .map_err(|_| "以前のLARM接続を解放できませんでした。".to_string())?;
        }
        let session = tokio::time::timeout(
            Duration::from_secs(180),
            connect_session(
                providers,
                credential,
                preference.clone(),
                scope,
                on_progress,
            ),
        )
        .await
        .map_err(|_| {
            "LARMが3分以内にモデルを準備できませんでした。LARM側のモデル状態を確認してください。"
                .to_string()
        })?
        .map_err(|error| {
            if error == "larm_startup_timeout" {
                "LARMがモデルを準備できませんでした。LARM側のモデル状態を確認してください。"
                    .to_string()
            } else {
                error
            }
        })?;
        *cached = Some(CachedTextSession {
            address: providers.harness.address.clone(),
            preference,
            credential: zeroize::Zeroizing::new(credential.to_string()),
            provider_scope: scope,
            session,
            generation: 0,
        });
    } else {
        on_progress("ready");
    }
    on_progress("provider_request");
    let session = Arc::clone(&cached.as_ref().expect("connected text session").session);
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        run_claimed(&session, capability, text, None, &providers.harness),
    )
    .await
    .map_err(|_| "Providerの応答が時間内に完了しませんでした。".to_string())
    .and_then(|result| result);
    let (model, output, audio_base64) = match result {
        Ok(result) => result,
        Err(error) => {
            if let Some(previous) = cached.take() {
                let _ = previous.session.close().await;
            }
            return Err(crate::redact::redact_runtime_text(&error));
        }
    };
    let generation = TEXT_SESSION_GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    cached.as_mut().expect("connected text session").generation = generation;
    drop(cached);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(180)).await;
        let mut cached = text_session().lock().await;
        if cached
            .as_ref()
            .is_some_and(|entry| entry.generation == generation)
        {
            if let Some(previous) = cached.take() {
                let _ = previous.session.close().await;
            }
        }
    });
    Ok(UnitTestResult {
        capability: capability.into(),
        model,
        output,
        latency_ms: started.elapsed().as_millis() as u64,
        audio_base64,
    })
}

async fn run_direct_tts(
    harness: &crate::HarnessSettings,
    credential: &str,
    preference: &saaa_larm_session::ProfilePreference,
    text: &str,
) -> Result<(String, String, Option<String>), String> {
    let base = url::Url::parse(&harness.address).map_err(|_| "LARMの接続先が不正です。")?;
    let selector = match preference {
        saaa_larm_session::ProfilePreference::Variant(variant) => variant.selector(),
        saaa_larm_session::ProfilePreference::Explicit(value) => value.as_str(),
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| "LARMへの接続を作成できませんでした。")?;
    let catalog = saaa_larm_session::catalog::fetch(&client, &base, credential, selector)
        .await
        .map_err(str::to_string)?;
    let provider = catalog
        .provider("tts")
        .ok_or("TTS Providerが見つかりません。")?;
    if provider.protocol != "openai.audio-speech.v1" || provider.endpoint != "/v1/audio/speech" {
        return Err("選択したTTSのプロトコルに対応していません。".into());
    }
    let voice = harness
        .tts_voice
        .as_deref()
        .or_else(|| (provider.model == "voicevox-core").then_some("Kasukabe_Tsumugi"))
        .ok_or("設定画面でTTSの音声を選択してください。")?;
    let mut settings = crate::CloudTtsProviderSettings {
        id: "larm-unit-tts".into(),
        enabled: true,
        label: "LARM TTS".into(),
        location: "local".into(),
        endpoint: base
            .join("v1")
            .map_err(|_| "LARMの接続先が不正です。")?
            .to_string(),
        model: provider.model.clone(),
        voice: voice.into(),
        response_format: "wav".into(),
        authentication: "api-key".into(),
        style: None,
        speed: None,
        pitch_scale: None,
        intonation_scale: None,
    };
    crate::voice::cloud_tts::speech_request::apply_harness_prosody(&mut settings, harness);
    let response = crate::voice::cloud_tts::request_audio_with_api_key(
        &settings,
        text,
        30_000,
        Arc::new(RunCancellation::default()),
        Some(credential),
    )
    .await?;
    crate::voice::cloud_tts::validate_audio_headers(&response, &settings.response_format)?;
    let mut stream = response.bytes_stream();
    let mut audio = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "TTSの音声取得が中断されました。")?;
        if audio.len().saturating_add(chunk.len()) > 2_000_000 {
            return Err("TTSの音声がテスト用の上限を超えました。".into());
        }
        audio.extend_from_slice(&chunk);
    }
    if audio.is_empty() {
        return Err("TTSから音声が返りませんでした。".into());
    }
    let audio_base64 = base64::engine::general_purpose::STANDARD.encode(&audio);
    Ok((
        provider.model.clone(),
        format!("音声を生成しました（{}バイト）", audio.len()),
        Some(audio_base64),
    ))
}

async fn run_claimed(
    session: &Arc<saaa_larm_session::Session>,
    capability: &str,
    text: &str,
    samples: Option<&[f32]>,
    harness: &crate::HarnessSettings,
) -> Result<(String, String, Option<String>), String> {
    if capability == "embedding" {
        let vectors = session
            .embed_query(&[text.to_string()])
            .await
            .map_err(str::to_string)?;
        let vector = vectors
            .first()
            .filter(|vector| !vector.is_empty())
            .ok_or("Embeddingからベクトルが返りませんでした。")?;
        let model = session
            .provider_summary()
            .await
            .into_iter()
            .find(|provider| provider.name == "embedding")
            .map(|provider| provider.model)
            .unwrap_or_else(|| "embedding".into());
        let preview = vector
            .iter()
            .take(8)
            .map(|value| format!("{value:.5}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Ok((
            model,
            format!("次元数: {}\n先頭8値: {preview}", vector.len()),
            None,
        ));
    }
    let lease = session.acquire(capability).await.map_err(str::to_string)?;
    let provider = lease.provider();
    let model = provider.model.clone();
    let budget = lease
        .request_budget(Duration::from_secs(110))
        .map_err(str::to_string)?;
    match capability {
        "llm" | "backchannel" => {
            if provider.protocol != "openai.chat-completions.v1" {
                return Err("選択したLLMのプロトコルに対応していません。".into());
            }
            let authorization = zeroize::Zeroizing::new(format!("Bearer {}", provider.token()));
            let output = super::conversation_check::complete_http(
                provider.base_url.as_str(),
                Some(authorization.as_str()),
                &model,
                text,
                budget.as_millis() as u64,
                None,
                true,
            )
            .await?;
            Ok((model, output, None))
        }
        "asr" => {
            if provider.protocol != "openai.audio-transcriptions.v1" {
                return Err("選択したASRのプロトコルに対応していません。".into());
            }
            let settings = crate::providers::larm_resources::audio::asr_settings(provider);
            let (output, language) = crate::voice::cloud_asr::transcribe_full(
                &settings,
                samples.ok_or("録音がありません。")?,
                16_000,
                budget.as_millis() as u64,
                Arc::new(RunCancellation::default()),
                Some(provider.token()),
            )
            .await?;
            let output = match language {
                Some(language) => format!("{output}\n\n検出言語: {language}"),
                None => output,
            };
            Ok((model, output, None))
        }
        "tts" => {
            let catalog_voice = if harness.tts_voice.is_none()
                && provider.voice.is_none()
                && provider.model == "voicevox-core"
            {
                let catalog = crate::voice::cloud_tts::tts_catalog::fetch_catalog(
                    provider.base_url.as_str(),
                    Some(provider.token()),
                    true,
                )
                .await?;
                catalog
                    .default_voice
                    .or_else(|| catalog.voices.first().map(|voice| voice.id.clone()))
            } else {
                None
            };
            let settings = crate::providers::larm_resources::audio::tts_settings(
                provider,
                harness.tts_voice.as_deref().or(catalog_voice.as_deref()),
                Some(harness),
            )?;
            let response = crate::voice::cloud_tts::request_audio_with_api_key(
                &settings,
                text,
                budget.as_millis() as u64,
                Arc::new(RunCancellation::default()),
                Some(provider.token()),
            )
            .await?;
            crate::voice::cloud_tts::validate_audio_headers(&response, &settings.response_format)?;
            let mut stream = response.bytes_stream();
            let mut audio = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| "TTSの音声取得が中断されました。")?;
                if audio.len().saturating_add(chunk.len()) > 2_000_000 {
                    return Err("TTSの音声がテスト用の上限を超えました。".into());
                }
                audio.extend_from_slice(&chunk);
            }
            if audio.is_empty() {
                return Err("TTSから音声が返りませんでした。".into());
            }
            let audio_base64 = base64::engine::general_purpose::STANDARD.encode(&audio);
            Ok((
                model,
                format!("音声を生成しました（{}バイト）", audio.len()),
                Some(audio_base64),
            ))
        }
        _ => Err("テスト対象のProviderが不正です。".into()),
    }
}

#[cfg(feature = "provider-unit-test-harness")]
pub async fn run_saved_asr_unit_test(
    database: &std::path::Path,
    samples: &[f32],
) -> Result<serde_json::Value, String> {
    let connection =
        rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| "診断用DBを読み取り専用で開けませんでした。")?;
    let settings = persistence::load_model_providers(&connection)?;
    let credential = crate::providers::dynamic_lan::credential::load()
        .map_err(|error| error.code().to_string())?;
    let result = run_with_settings(
        &settings,
        credential.token(),
        "asr",
        "",
        Some(samples),
        false,
        &|stage| eprintln!("ASR diagnostic stage: {stage}"),
    )
    .await?;
    serde_json::to_value(result).map_err(|_| "テスト結果を変換できませんでした。".into())
}

#[cfg(feature = "provider-unit-test-harness")]
pub async fn run_fixture_provider_unit_test(
    address: &str,
    profile: &str,
    voice: &str,
    credential: &str,
    capability: &str,
    text: &str,
    samples: Option<&[f32]>,
) -> Result<serde_json::Value, String> {
    run_fixture_provider_unit_test_mode(
        address, profile, voice, credential, capability, text, samples, false,
    )
    .await
}

#[cfg(feature = "provider-unit-test-harness")]
pub async fn run_fixture_provider_unit_test_cached(
    address: &str,
    profile: &str,
    credential: &str,
    capability: &str,
    text: &str,
) -> Result<serde_json::Value, String> {
    run_fixture_provider_unit_test_mode(
        address, profile, "", credential, capability, text, None, true,
    )
    .await
}

#[cfg(feature = "provider-unit-test-harness")]
pub async fn release_cached_provider_unit_test_session() -> Result<(), String> {
    if let Some(previous) = text_session().lock().await.take() {
        previous.session.close().await.map_err(str::to_string)?;
    }
    Ok(())
}

#[cfg(feature = "provider-unit-test-harness")]
async fn run_fixture_provider_unit_test_mode(
    address: &str,
    profile: &str,
    voice: &str,
    credential: &str,
    capability: &str,
    text: &str,
    samples: Option<&[f32]>,
    reuse_text_session: bool,
) -> Result<serde_json::Value, String> {
    let settings = crate::ModelProvidersSettings {
        harness: crate::HarnessSettings {
            address: address.into(),
            larm_profile: Some(profile.into()),
            tts_voice: (!voice.is_empty()).then(|| voice.into()),
            tts_style: None,
            tts_speed: None,
            tts_pitch_scale: None,
            tts_intonation_scale: None,
        },
        providers: Vec::new(),
        reasoning_effort: "medium".into(),
    };
    let result = run_with_settings(
        &settings,
        credential,
        capability,
        text,
        samples,
        reuse_text_session,
        &|stage| {
            eprintln!("{capability} diagnostic stage: {stage}");
        },
    )
    .await?;
    serde_json::to_value(result).map_err(|_| "テスト結果を変換できませんでした。".into())
}
