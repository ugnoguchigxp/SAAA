//! Opt-in fixture for the real conversation queue workers. No saved user data is opened.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

use axum::{
    body::to_bytes,
    extract::{Request, State},
    http::{header, Method, StatusCode},
    response::{IntoResponse, Response},
    Json, Router,
};
use futures_util::StreamExt;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use tauri::Manager;

#[path = "conversation_queue_e2e/context_trial.rs"]
mod context_trial;
#[path = "conversation_queue_e2e/server.rs"]
mod server;
use server::serve;
#[path = "conversation_queue_e2e/cloud_route.rs"]
mod cloud_route;
#[path = "conversation_queue_e2e/scenario.rs"]
mod scenario;
use scenario::run_with_server;
#[path = "conversation_queue_e2e/context_live.rs"]
mod context_live;
pub async fn run_live_context_trial(mode: &str) -> Result<Value, String> {
    context_live::run(mode).await
}

#[path = "conversation_queue_e2e/cancellation.rs"]
mod cancellation;
#[path = "conversation_queue_e2e/dictionary.rs"]
mod dictionary;
#[path = "conversation_queue_e2e/dictionary_live.rs"]
mod dictionary_live;

pub async fn run_live_tts_dictionary() -> Result<Value, String> {
    dictionary_live::run().await
}
#[path = "tts_dictionary/verification.rs"]
mod dictionary_verification;

pub fn verify_tts_dictionary_tools() -> Result<Value, String> {
    dictionary_verification::run()
}

use crate::{
    persistence, runtime::conversation_check, task_queue, AppState, PRIMARY_CONVERSATION_ID,
};

/// Operator-only check of the actual local output callback. Uses no microphone or TTS provider.
pub fn probe_idle_audio_output() -> Result<serde_json::Value, String> {
    use crate::voice::local_audio_output as output;
    output::start_idle_output()?;
    struct Stop;
    impl Drop for Stop {
        fn drop(&mut self) {
            crate::voice::local_audio_output::set_speaking(false);
            crate::voice::local_audio_output::stop_idle_output();
        }
    }
    let _stop = Stop;
    std::thread::sleep(std::time::Duration::from_millis(300));
    let first = serde_json::to_value(output::idle_output_status()).map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(300));
    let second = serde_json::to_value(output::idle_output_status()).map_err(|e| e.to_string())?;
    output::set_speaking(true);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let paused = serde_json::to_value(output::idle_output_status()).map_err(|e| e.to_string())?;
    std::thread::sleep(std::time::Duration::from_millis(150));
    let paused_again =
        serde_json::to_value(output::idle_output_status()).map_err(|e| e.to_string())?;
    output::set_speaking(false);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let resumed = serde_json::to_value(output::idle_output_status()).map_err(|e| e.to_string())?;
    Ok(json!({"first": first, "second": second, "paused": paused,
        "pausedAgain": paused_again, "resumed": resumed}))
}

pub fn inspect_system_wav(bytes: &[u8]) -> Result<(u32, u16, usize), String> {
    let mut decoder = crate::voice::http_audio::decode::Decoder::new("wav")?;
    let samples = decoder.push(bytes)?;
    decoder.finish()?;
    let format = decoder.format.ok_or("WAV format is missing")?;
    Ok((format.rate, format.channels, samples.len()))
}

/// Exercises the actual local PCM output with a silent WAV; it produces no audible sound.
pub async fn probe_silent_pcm_playback() -> Result<bool, String> {
    probe_silent_pcm_playback_inner(false).await
}

pub async fn probe_vpio_handoff_to_silent_pcm() -> Result<bool, String> {
    probe_silent_pcm_playback_inner(true).await
}

async fn probe_silent_pcm_playback_inner(force_vpio: bool) -> Result<bool, String> {
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let path = directory.path().join("silent.wav");
    let wave = crate::voice::network_asr::encode_wav(&[0.0; 4_800], 24_000)?;
    std::fs::write(&path, wave).map_err(|error| error.to_string())?;
    let cancellation = Arc::new(crate::RunCancellation::default());
    let started = Arc::new(AtomicBool::new(false));
    let started_callback = started.clone();
    let on_started = move || started_callback.store(true, Ordering::Release);
    let player = if force_vpio {
        crate::voice::local_audio_output::ContinuousPlayback::start_vpio_handoff_probe(
            cancellation.clone(),
            on_started,
        )
    } else {
        crate::voice::local_audio_output::ContinuousPlayback::start(
            cancellation.clone(),
            on_started,
        )
    };
    player.play_wav_file(&path, &cancellation).await?;
    player.finish().await?;
    Ok(started.load(Ordering::Acquire))
}

#[derive(Default)]
struct Fixture {
    context_trial: AtomicBool,
    context_writer: Mutex<Option<Arc<persistence::SqliteWriter>>>,
    requests: Mutex<Vec<Value>>,
    calls: Mutex<Vec<String>>,
    spoken: Mutex<Vec<String>>,
    spoken_times: Mutex<Vec<std::time::Instant>>,
    searches: Mutex<Vec<String>>,
    llm_calls: Mutex<usize>,
    speech_before_done: AtomicBool,
    fail_after_search: AtomicBool,
    revoke_cloud: AtomicBool,
    cloud_deadline: AtomicBool,
    cloud_fallback: AtomicBool,
    cloud_native: AtomicBool,
    cloud_options: AtomicBool,
    cloud_auth_rejection: AtomicBool,
    withdraw_cloud: AtomicBool,
    switch_cloud: AtomicBool,
    slow_ornith: AtomicBool,
    invalid_reply: AtomicBool,
    authentication_failure: AtomicBool,
    asr_no_speech: AtomicBool,
    live_dictionary: AtomicBool,
    cancellation: cancellation::Control,
}

static ACTIVE: OnceLock<Mutex<Option<Arc<Fixture>>>> = OnceLock::new();
fn active() -> &'static Mutex<Option<Arc<Fixture>>> {
    ACTIVE.get_or_init(|| Mutex::new(None))
}
fn fixture() -> Option<Arc<Fixture>> {
    active().lock().ok()?.clone()
}

pub(crate) fn credential() -> Option<String> {
    fixture()
        .filter(|fixture| !fixture.live_dictionary.load(Ordering::SeqCst))
        .map(|_| "fixture-control-token".into())
}
pub(crate) fn capture_speech(text: &str) -> bool {
    let Some(fixture) = fixture() else {
        return false;
    };
    fixture
        .spoken
        .lock()
        .expect("fixture spoken lock")
        .push(text.into());
    fixture
        .spoken_times
        .lock()
        .expect("fixture time lock")
        .push(std::time::Instant::now());
    true
}
pub(crate) fn web_search(query: &str) -> Option<String> {
    let fixture = fixture()?;
    if fixture.revoke_cloud.swap(false, Ordering::SeqCst)
        || fixture.withdraw_cloud.load(Ordering::SeqCst)
        || fixture.switch_cloud.load(Ordering::SeqCst)
    {
        fixture
            .context_writer
            .lock()
            .expect("cloud writer")
            .as_ref()
            .expect("cloud writer set")
            .write(|db| {
                let loaded = persistence::service_registry_store::load_registry(db)?;
                let mut snapshot = loaded.snapshot;
                if fixture.withdraw_cloud.load(Ordering::SeqCst) {
                    snapshot
                        .bindings
                        .iter_mut()
                        .find(|b| {
                            b.purpose
                                == crate::providers::service_registry::Purpose::ConversationRespond
                        })
                        .ok_or("binding missing")?
                        .cloud_allowed = false;
                } else if fixture.switch_cloud.load(Ordering::SeqCst) {
                    let binding = snapshot
                        .bindings
                        .iter_mut()
                        .find(|b| {
                            b.purpose
                                == crate::providers::service_registry::Purpose::ConversationRespond
                        })
                        .ok_or("binding missing")?;
                    binding.primary_resource_id = Some("res:harness-llm".into());
                    binding.fallback_resource_ids.clear();
                } else {
                    snapshot
                        .connections
                        .iter_mut()
                        .find(|c| c.connection_id == "conn:svc-cloud-llm")
                        .ok_or("cloud connection missing")?
                        .enabled = false;
                }
                persistence::service_registry_store::save_registry(db, &snapshot, loaded.revision)?;
                Ok(())
            })
            .expect("cloud revocation saved");
    }
    fixture
        .searches
        .lock()
        .expect("fixture search lock")
        .push(query.into());
    Some(json!({"hits":[
        {"title":"Unrelated","url":"https://example.invalid/other","snippet":"別の資料です。"},
        {"title":"Fixture report","url":"https://example.invalid/report","snippet":"確認済みの事実は42です。"}
    ]}).to_string())
}

pub(crate) fn fetch_content(url: &str) -> Option<String> {
    let _fixture = fixture()?;
    if url.ends_with("/other") {
        return Some(
            json!({"error":{"code":"FETCH_FAILED","message":"fixture page unavailable"}})
                .to_string(),
        );
    }
    Some(json!({"document":{"url":url,"text":format!("{} 確認済みの事実は42です。", "\"".repeat(4000)),
        "retrievalStatus":"relevant","truncated":false},"security":{"trust":"untrusted"}}).to_string())
}

fn declaration(name: &str) -> Value {
    let (capability, protocol, endpoint, model) = match name {
        "asr" => (
            "speech.stt",
            "openai.audio-transcriptions.v1",
            "/v1/audio/transcriptions",
            "fixture-asr",
        ),
        "backchannel" => (
            "llm.backchannel.classifier",
            "openai.chat-completions.v1",
            "/v1/chat/completions",
            "fixture-qwen",
        ),
        "llm" => (
            "llm.general",
            "openai.chat-completions.v1",
            "/v1/chat/completions",
            "fixture-ornith",
        ),
        "tts" => (
            "speech.tts",
            "openai.audio-speech.v1",
            "/v1/audio/speech",
            "fixture-tts",
        ),
        "embedding" => (
            "embedding",
            "larm.embedding.v1",
            "/v1/embed",
            "fixture-embedding",
        ),
        _ => unreachable!(),
    };
    let mut value = json!({"name":name,"capability":capability,"protocol":protocol,"endpoint":endpoint,"model":model});
    if matches!(name, "llm" | "backchannel") {
        value["contextWindow"] =
            json!({"maxTokens":65536,"outputReserveTokens":4096,"safetyMarginTokens":1976});
    }
    value
}

fn state_value() -> Value {
    let providers = ["asr", "llm", "tts", "embedding"]
        .iter()
        .map(|name| {
            let mut value = declaration(name);
            value["readiness"] = json!("ready");
            value["claimable"] = json!(true);
            value
        })
        .collect::<Vec<_>>();
    json!({"id":"session-1","profile":"SAAA","agentProfile":"saaa-conversation-ornith15",
        "providers":providers,"services":[],"status":"ready",
        "expiresAt":(chrono::Utc::now()+chrono::Duration::minutes(15)).to_rfc3339()})
}

fn claim_value(base: &str) -> Value {
    let providers = ["asr", "llm", "tts", "embedding"]
        .iter()
        .map(|name| {
            let mut value = declaration(name);
            let url = format!("{base}/{name}/v1");
            value["baseUrl"] = json!(url);
            value["credential"] = json!({"token":format!("token-{name}")});
            let url_field = if *name == "embedding" {
                "daemonURL"
            } else {
                "baseURL"
            };
            let mut fields = json!({"model":value["model"]});
            fields[url_field] = json!(url);
            value["configuration"] = json!({"fields":fields});
            value["health"] = json!({"url":format!("{base}/{name}/health"),"maxAgeMs":10000});
            if *name == "embedding" {
                value["embeddingSpace"] = json!({"dimension":384});
            }
            value
        })
        .collect::<Vec<_>>();
    let mut value = state_value();
    value["allocationId"] = json!("allocation-1");
    value["providers"] = json!(providers);
    value
}

struct FixtureGuard;
impl Drop for FixtureGuard {
    fn drop(&mut self) {
        *active().lock().expect("active fixture lock") = None;
    }
}

pub fn verify_legacy_queue_migration() -> Result<(), String> {
    let db = rusqlite::Connection::open_in_memory().map_err(crate::database_error)?;
    task_queue::migrate(&db).map_err(crate::database_error)?;
    let scope = "legacy";
    let key = "input";
    let payload = json!({"text":"調べて"}).to_string();
    let old_input = task_queue::enqueue(&db, scope, "qwen", "user_input", key, 0, &payload, None)?;
    db.execute(
        "UPDATE task_queue_jobs SET state='completed' WHERE id=?1",
        [&old_input],
    )
    .map_err(crate::database_error)?;
    task_queue::enqueue(&db, scope, "ornith", "ornith_task", key, 0, &payload, None)?;
    conversation_check::queue_runtime::migrate_legacy_jobs(&db)?;
    let rows: Vec<(String, String, String)> = {
        let mut query = db
            .prepare("SELECT lane,kind,state FROM task_queue_jobs ORDER BY rowid")
            .map_err(crate::database_error)?;
        let rows = query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(crate::database_error)?
            .collect::<Result<_, _>>()
            .map_err(crate::database_error)?;
        rows
    };
    if rows
        != [
            ("qwen".into(), "user_input".into(), "completed".into()),
            (
                "conversation".into(),
                "conversation_resume".into(),
                "queued".into(),
            ),
        ]
    {
        return Err(format!("legacy queue migration mismatch: {rows:?}"));
    }
    Ok(())
}

pub async fn run() -> Result<Value, String> {
    run_variant(false, false, false, false, false).await
}

pub async fn run_cloud_conversation() -> Result<Value, String> {
    run_variant_with(false, false, false, false, false, Some(CloudCase::Normal)).await
}

#[derive(Clone, Copy)]
enum CloudCase {
    Normal,
    Revoked,
    Deadline,
    Fallback,
    AuthRejected,
    Withdrawn,
    Switched,
    Native,
    Options,
}

pub async fn run_cloud_boundary(case: &str) -> Result<Value, String> {
    let case = match case {
        "revoked" => CloudCase::Revoked,
        "deadline" => CloudCase::Deadline,
        "fallback" => CloudCase::Fallback,
        "auth" => CloudCase::AuthRejected,
        "consent" => CloudCase::Withdrawn,
        "switch" => CloudCase::Switched,
        "native" => CloudCase::Native,
        "options" => CloudCase::Options,
        _ => return Err("Unknown cloud boundary case".into()),
    };
    run_variant_with(false, false, false, false, false, Some(case)).await
}

pub async fn run_failure_after_search() -> Result<Value, String> {
    run_variant(true, false, false, false, false).await
}

pub async fn run_waiting() -> Result<Value, String> {
    run_variant(false, true, false, false, false).await
}

pub async fn run_invalid_reply() -> Result<Value, String> {
    run_variant(false, false, true, false, false).await
}

pub async fn run_authentication_failure() -> Result<Value, String> {
    run_variant(false, false, false, true, false).await
}

pub async fn run_context_trial(mode: &str) -> Result<Value, String> {
    if !matches!(mode, "legacy" | "stable") {
        return Err("invalid trial mode".into());
    }
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(value) = &self.0 {
                std::env::set_var("SAAA_CONVERSATION_PREFIX_MODE", value);
            } else {
                std::env::remove_var("SAAA_CONVERSATION_PREFIX_MODE");
            }
        }
    }
    let _restore = Restore(std::env::var_os("SAAA_CONVERSATION_PREFIX_MODE"));
    std::env::set_var("SAAA_CONVERSATION_PREFIX_MODE", mode);
    run_variant(false, false, false, false, true).await
}

async fn run_variant(
    fail_after_search: bool,
    slow_ornith: bool,
    invalid_reply: bool,
    authentication_failure: bool,
    context_trial: bool,
) -> Result<Value, String> {
    run_variant_with(
        fail_after_search,
        slow_ornith,
        invalid_reply,
        authentication_failure,
        context_trial,
        None,
    )
    .await
}

async fn run_variant_with(
    fail_after_search: bool,
    slow_ornith: bool,
    invalid_reply: bool,
    authentication_failure: bool,
    context_trial: bool,
    cloud_route: Option<CloudCase>,
) -> Result<Value, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| error.to_string())?;
    let base = format!(
        "http://{}",
        listener.local_addr().map_err(|error| error.to_string())?
    );
    let fixture = Arc::new(Fixture::default());
    fixture.context_trial.store(context_trial, Ordering::SeqCst);
    fixture.invalid_reply.store(invalid_reply, Ordering::SeqCst);
    fixture
        .authentication_failure
        .store(authentication_failure, Ordering::SeqCst);
    fixture
        .fail_after_search
        .store(fail_after_search, Ordering::SeqCst);
    fixture.slow_ornith.store(slow_ornith, Ordering::SeqCst);
    fixture.revoke_cloud.store(
        matches!(cloud_route, Some(CloudCase::Revoked)),
        Ordering::SeqCst,
    );
    fixture.cloud_deadline.store(
        matches!(cloud_route, Some(CloudCase::Deadline)),
        Ordering::SeqCst,
    );
    fixture.cloud_options.store(
        matches!(cloud_route, Some(CloudCase::Options)),
        Ordering::SeqCst,
    );
    fixture.cloud_native.store(
        matches!(cloud_route, Some(CloudCase::Native)),
        Ordering::SeqCst,
    );
    fixture.cloud_fallback.store(
        matches!(
            cloud_route,
            Some(CloudCase::Fallback | CloudCase::AuthRejected)
        ),
        Ordering::SeqCst,
    );
    fixture.cloud_auth_rejection.store(
        matches!(cloud_route, Some(CloudCase::AuthRejected)),
        Ordering::SeqCst,
    );
    fixture.withdraw_cloud.store(
        matches!(cloud_route, Some(CloudCase::Withdrawn)),
        Ordering::SeqCst,
    );
    fixture.switch_cloud.store(
        matches!(cloud_route, Some(CloudCase::Switched)),
        Ordering::SeqCst,
    );
    *active().lock().map_err(|_| "fixture lock unavailable")? = Some(fixture.clone());
    let _guard = FixtureGuard;
    let router = Router::new()
        .fallback(serve)
        .with_state((fixture.clone(), base.clone()));
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let result = if cloud_route.is_some() {
        cloud_route::run_with_server(&base, &fixture).await
    } else {
        run_with_server(&base, &fixture).await
    };
    if result.is_err() {
        eprintln!(
            "conversation queue fixture calls: {:?}",
            fixture.calls.lock().ok()
        );
    }
    conversation_check::reset_fixture_asr_session().await;
    server.abort();
    result
}

pub async fn run_live_web_retrieval() -> Result<Value, String> {
    crate::runtime::web_fetch::live_retrieval().await
}
