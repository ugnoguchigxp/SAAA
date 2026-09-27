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

#[path = "conversation_queue_e2e/cancellation.rs"]
mod cancellation;

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
            cancellation.clone(), on_started)
    } else {
        crate::voice::local_audio_output::ContinuousPlayback::start(
            cancellation.clone(), on_started)
    };
    player.play_wav_file(&path, &cancellation).await?;
    player.finish().await?;
    Ok(started.load(Ordering::Acquire))
}

#[derive(Default)]
struct Fixture {
    calls: Mutex<Vec<String>>,
    spoken: Mutex<Vec<String>>,
    spoken_times: Mutex<Vec<std::time::Instant>>,
    searches: Mutex<Vec<String>>,
    llm_calls: Mutex<usize>,
    speech_before_done: AtomicBool,
    fail_after_search: AtomicBool,
    slow_ornith: AtomicBool,
    invalid_reply: AtomicBool,
    authentication_failure: AtomicBool,
    asr_no_speech: AtomicBool,
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
    fixture().map(|_| "fixture-control-token".into())
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

async fn serve(
    State((fixture, base)): State<(Arc<Fixture>, String)>,
    request: Request,
) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    fixture
        .calls
        .lock()
        .expect("fixture calls lock")
        .push(format!("{method} {path}"));
    if path == "/v3/agent-profiles" {
        return Json(json!({"contractVersion":"agent-connection.v3","catalogRevision":"queue-e2e",
            "audiences":["saaa-desktop"],
            "requestedProfile":"SAAA","profiles":[{"id":"saaa-conversation-ornith15",
            "providers":[declaration("asr"),declaration("llm"),declaration("tts"),declaration("embedding")],"services":[]}]})).into_response();
    }
    if path.starts_with("/v1/agent-connections") {
        if request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            != Some("Bearer fixture-control-token")
        {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if method == Method::DELETE {
            return StatusCode::NO_CONTENT.into_response();
        }
        if path.ends_with("/claim") {
            return Json(claim_value(&base)).into_response();
        }
        if method == Method::POST {
            return (StatusCode::CREATED, Json(state_value())).into_response();
        }
        return Json(state_value()).into_response();
    }
    if path.ends_with("/health") {
        return Json(json!({"ready":true,"acceptingRequests":true,
            "capacity":{"maxConcurrentRequests":4,"activeRequests":0,"maxQueuedRequests":4,
            "queueDepth":0,"queueTimeoutMs":1000,"retryAfterMs":0,"completionGuaranteed":false},
            "probe":{"validated":true,"protocol":if path.starts_with("/asr/") {"openai.audio-transcriptions.v1"} else if path.starts_with("/tts/") {"openai.audio-speech.v1"} else if path.starts_with("/embedding/") {"larm.embedding.v1"} else {"openai.chat-completions.v1"}}})).into_response();
    }
    if path == "/asr/v1/audio/transcriptions" {
        let bytes = to_bytes(request.into_body(), 1_000_000)
            .await
            .expect("ASR fixture body");
        if !bytes.windows(4).any(|part| part == b"RIFF") {
            return StatusCode::BAD_REQUEST.into_response();
        }
        if fixture.asr_no_speech.load(Ordering::SeqCst) {
            return Json(json!({"text":"こんにちは。","language":"ja",
                "segments":[{"no_speech_prob":0.95}]}))
            .into_response();
        }
        return Json(json!({"text":"今日の事実を調べて","language":"ja"})).into_response();
    }
    if path.ends_with("/v1/chat/completions") {
        let bytes = to_bytes(request.into_body(), 1_000_000)
            .await
            .expect("LLM fixture body");
        let body: Value = serde_json::from_slice(&bytes).expect("LLM fixture JSON");
        let serialized = body.to_string();
        let content = if let Some(content) = cancellation::respond(&fixture, &path, &body).await {
            content
        } else if path.starts_with("/llm/") {
            if fixture.authentication_failure.load(Ordering::SeqCst) {
                *fixture.llm_calls.lock().expect("LLM fixture count") += 1;
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let first_call = *fixture.llm_calls.lock().expect("LLM fixture count") == 0;
            if first_call && fixture.slow_ornith.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(11_000)).await;
            }
            let mut count = fixture.llm_calls.lock().expect("LLM fixture count");
            if *count == 0 {
                assert_eq!(
                    body["max_tokens"], 4_096,
                    "initial Ornith call keeps its output reserve"
                );
            } else {
                assert_eq!(
                    body["max_tokens"], 4_096,
                    "tool follow-up keeps the advertised output reserve"
                );
                if serialized.contains("TOOL_RESULT:") {
                    assert_ne!(body["chat_template_kwargs"]["enable_thinking"], false);
                }
            }
            if *count > 0 && fixture.fail_after_search.load(Ordering::SeqCst) {
                *count += 1;
                if body["stream"] == true {
                    let event = json!({"model":body["model"],"choices":[{"index":0,"delta":{},"finish_reason":"length"}]});
                    return ([(header::CONTENT_TYPE, "text/event-stream")],
                        format!("data: {event}\n\ndata: [DONE]\n\n")).into_response();
                }
                return Json(json!({"choices":[{"message":{"content":""},"finish_reason":"length"}]})).into_response();
            }
            assert!(
                serialized.contains(&chrono::Local::now().format("%Y-%m-%d").to_string()),
                "runtime date reaches Ornith"
            );
            let current_user_text = body["messages"]
                .as_array()
                .and_then(|messages| messages.iter().rev().find(|message| message["role"] == "user"))
                .and_then(|message| message["content"].as_str())
                .unwrap_or_default();
            let result = if current_user_text == "こんにちは" {
                json!({"action":"answer","content":"こんにちは。","sources":[]}).to_string()
            } else { match *count {
                0 => json!({"action":"web_search","query":"fixture fact"}).to_string(),
                1 => {
                    assert!(
                        body["messages"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|m| m["role"] == "assistant"
                                && m["content"]
                                    .as_str()
                                    .is_some_and(|s| s.contains("web_search"))),
                        "search action is retained in dialogue"
                    );
                    json!({"action":"fetch_content","url":"https://example.invalid/other"})
                        .to_string()
                }
                2 => {
                    assert!(serialized.contains("FETCH_FAILED"));
                    json!({"action":"fetch_content","url":"https://example.invalid/report"})
                        .to_string()
                }
                _ => {
                    let fetched = body["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .rev()
                        .filter_map(|m| m["content"].as_str())
                        .find(|s| s.starts_with("[TOOL_RESULT: fetch_content"))
                        .unwrap();
                    let document: Value = serde_json::from_str(fetched.split_once('\n').unwrap().1)
                        .expect("tool JSON must not be cut in half");
                    assert!(document["document"]["text"]
                        .as_str()
                        .unwrap()
                        .contains("確認済みの事実は42"));
                    let answer = if fixture.invalid_reply.load(Ordering::SeqCst) {
                        "保存してはいけない回答<think>制御文</think>"
                    } else {
                        "資料では確認済みの事実は42です。"
                    };
                    json!({"action":"answer","content":answer,"sources":["https://example.invalid/report"]}).to_string()
                }
            }};
            *count += 1;
            result
        } else if serialized.contains("会話の入口") {
            assert_eq!(body["max_tokens"], 4_096);
            json!({"route":"think","reply":null}).to_string()
        } else if serialized.contains("挨拶に自然な日本語") {
            "こんにちは。".into()
        } else {
            assert_eq!(body["max_tokens"], 4_096);
            assert!(serialized.contains("ORNITH_RESULT"));
            assert!(
                !serialized.contains("古い天気は雨です"),
                "Qwen speaker must not read stale conversation replies"
            );
            if fixture.authentication_failure.load(Ordering::SeqCst) {
                assert!(serialized.contains("Provider authentication failed"));
                "接続の認証に失敗したため回答できませんでした。".into()
            } else if fixture.invalid_reply.load(Ordering::SeqCst) {
                "保存してはいけない回答<think>制御文</think>".into()
            } else if fixture.fail_after_search.load(Ordering::SeqCst) {
                assert!(
                    serialized.contains("結果を整理する段階で失敗"),
                    "Qwen must read the Ornith failure"
                );
                "調査結果を確定できませんでした。".into()
            } else {
                assert!(
                    serialized.contains("確認済みの事実は42"),
                    "Qwen must read Ornith result"
                );
                "調査結果は42です。補足です。これ以上の説明は不要です。".into()
            }
        };
        if body["stream"] == true {
            let model = body["model"].as_str().unwrap_or("fixture-ornith");
            if let Some(split_at) = content.find("資料では確認済みの事実は42です。") {
                let split_at = split_at + "資料では確認済みの事実は42です。".len();
                let first_event = json!({"model":model,"choices":[{"index":0,"delta":{"content":&content[..split_at]},"finish_reason":null}]});
                let second_event = json!({"model":model,"choices":[{"index":0,"delta":{"content":&content[split_at..]},"finish_reason":"stop"}]});
                let first = format!("data: {first_event}\n\n");
                let second = format!("data: {second_event}\n\ndata: [DONE]\n\n");
                let before_done = fixture.clone();
                let chunks = futures_util::stream::once(async move {
                    Ok::<_, std::io::Error>(axum::body::Bytes::from(first))
                }).chain(futures_util::stream::once(async move {
                    let first_speech = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                        loop {
                            if before_done.spoken.lock().expect("fixture spoken lock").iter()
                                .any(|text| text == "シリョウではカクニン済みのジジツは42です。") { break; }
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                    }).await.is_ok();
                    before_done.speech_before_done.store(first_speech, Ordering::SeqCst);
                    Ok::<_, std::io::Error>(axum::body::Bytes::from(second))
                }));
                return ([(header::CONTENT_TYPE, "text/event-stream")],
                    axum::body::Body::from_stream(chunks)).into_response();
            }
            let event = json!({"model":model,"choices":[{"index":0,"delta":{"content":content},"finish_reason":"stop"}]});
            let stream = format!("data: {event}\n\ndata: [DONE]\n\n");
            return ([(header::CONTENT_TYPE, "text/event-stream")], stream).into_response();
        }
        return Json(json!({"choices":[{"message":{"content":content},"finish_reason":"stop"}]}))
            .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
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
    db.execute("UPDATE task_queue_jobs SET state='completed' WHERE id=?1", [&old_input])
        .map_err(crate::database_error)?;
    task_queue::enqueue(&db, scope, "ornith", "ornith_task", key, 0, &payload, None)?;
    conversation_check::queue_runtime::migrate_legacy_jobs(&db)?;
    let rows: Vec<(String, String, String)> = {
        let mut query = db.prepare("SELECT lane,kind,state FROM task_queue_jobs ORDER BY rowid")
            .map_err(crate::database_error)?;
        let rows = query.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(crate::database_error)?
            .collect::<Result<_, _>>().map_err(crate::database_error)?;
        rows
    };
    if rows != [("qwen".into(), "user_input".into(), "completed".into()),
                ("conversation".into(), "conversation_resume".into(), "queued".into())] {
        return Err(format!("legacy queue migration mismatch: {rows:?}"));
    }
    Ok(())
}

pub async fn run() -> Result<Value, String> {
    run_variant(false, false, false, false).await
}

pub async fn run_failure_after_search() -> Result<Value, String> {
    run_variant(true, false, false, false).await
}

pub async fn run_waiting() -> Result<Value, String> {
    run_variant(false, true, false, false).await
}

pub async fn run_invalid_reply() -> Result<Value, String> {
    run_variant(false, false, true, false).await
}

pub async fn run_authentication_failure() -> Result<Value, String> {
    run_variant(false, false, false, true).await
}

async fn run_variant(
    fail_after_search: bool,
    slow_ornith: bool,
    invalid_reply: bool,
    authentication_failure: bool,
) -> Result<Value, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| error.to_string())?;
    let base = format!(
        "http://{}",
        listener.local_addr().map_err(|error| error.to_string())?
    );
    let fixture = Arc::new(Fixture::default());
    fixture.invalid_reply.store(invalid_reply, Ordering::SeqCst);
    fixture
        .authentication_failure
        .store(authentication_failure, Ordering::SeqCst);
    fixture
        .fail_after_search
        .store(fail_after_search, Ordering::SeqCst);
    fixture.slow_ornith.store(slow_ornith, Ordering::SeqCst);
    *active().lock().map_err(|_| "fixture lock unavailable")? = Some(fixture.clone());
    let _guard = FixtureGuard;
    let router = Router::new()
        .fallback(serve)
        .with_state((fixture.clone(), base.clone()));
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let result = run_with_server(&base, &fixture).await;
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

async fn run_with_server(base: &str, fixture: &Arc<Fixture>) -> Result<Value, String> {
    let connection = Connection::open_in_memory().map_err(crate::database_error)?;
    persistence::schema::initialize_database(&connection).map_err(crate::database_error)?;
    let mut providers: Value = connection.query_row(
        "SELECT value_json FROM settings_documents WHERE namespace='providers.model' AND key='default'",
        [], |row| row.get::<_, String>(0),
    ).map_err(crate::database_error).and_then(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))?;
    providers["harness"]["address"] = json!(base);
    providers["harness"]["ttsVoice"] = json!("fixture-voice");
    connection.execute("UPDATE settings_documents SET value_json=?1 WHERE namespace='providers.model' AND key='default'",
        [providers.to_string()]).map_err(crate::database_error)?;
    let mut routing: Value = connection.query_row(
        "SELECT value_json FROM settings_documents WHERE namespace='routing.tasks' AND key='default'",
        [], |row| row.get::<_, String>(0),
    ).map_err(crate::database_error).and_then(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))?;
    routing["voiceTranscribe"]["source"] = json!("harness");
    routing["voiceTranscribe"]["providerId"] = Value::Null;
    routing["voiceSpeak"]["source"] = json!("harness");
    routing["voiceSpeak"]["providerId"] = Value::Null;
    connection.execute("UPDATE settings_documents SET value_json=?1 WHERE namespace='routing.tasks' AND key='default'",
        [routing.to_string()]).map_err(crate::database_error)?;
    let state = crate::test_support::app_state(connection);
    let app = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .map_err(|error| error.to_string())?;
    let state = app.state::<AppState>();
    state.sqlite_writer.write(|connection| {
        connection
            .execute(
                "INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
             VALUES('fixture_stale_reply',?1,'assistant','古い天気は雨です','1')",
                [PRIMARY_CONVERSATION_ID],
            )
            .map_err(crate::database_error)?;
        Ok(())
    })?;
    let input_id = "queue-e2e-main";
    fixture.asr_no_speech.store(true, Ordering::SeqCst);
    let non_speech =
        conversation_check::transcribe_fixture_audio(&state, "queue-e2e-no-speech").await;
    fixture.asr_no_speech.store(false, Ordering::SeqCst);
    if !non_speech.is_err_and(|error| error.starts_with("ASR_NO_SPEECH:")) {
        return Err("non-speech ASR text was accepted as a user utterance".into());
    }
    let transcript = conversation_check::transcribe_fixture_audio(&state, input_id).await?;
    if transcript != "今日の事実を調べて" {
        return Err(format!("ASR transcript mismatch: {transcript}"));
    }
    let receipt = conversation_check::queue_runtime::enqueue_text(
        &state,
        input_id.into(),
        transcript.clone(),
    )?;
    let duplicate = conversation_check::queue_runtime::enqueue_text(
        &state,
        input_id.into(),
        transcript.clone(),
    )?;
    if receipt.job_id != duplicate.job_id {
        return Err("ASR delivery was not idempotent".into());
    }
    conversation_check::spawn_queue_workers(app.handle().clone());
    state.conversation_queue_wake.notify_waiters();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let done = state.sqlite_readers.read(|connection| {
            let state: Option<String> = connection.query_row(
                "SELECT state FROM task_queue_jobs WHERE scope=?1 AND kind='speech' AND job_key=?2",
                rusqlite::params![PRIMARY_CONVERSATION_ID,input_id], |row| row.get(0),
            ).optional().map_err(crate::database_error)?;
            Ok(state)
        })?;
        if fixture.invalid_reply.load(Ordering::SeqCst) {
            let rejected: bool = state.sqlite_readers.read(|connection| {
                connection.query_row("SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE job_key=?1 AND kind='user_input' AND state='failed')",
                    [input_id], |row| row.get(0)).map_err(crate::database_error)
            })?;
            if rejected {
                let saved: bool = state.sqlite_readers.read(|connection| {
                    connection
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id=?1)",
                            [format!("reply_{input_id}")],
                            |row| row.get(0),
                        )
                        .map_err(crate::database_error)
                })?;
                let pending_progress: i64 = state.sqlite_readers.read(|connection| {
                    connection.query_row(
                        "SELECT count(*) FROM task_queue_jobs WHERE job_key=?1 AND kind='progress_speech' AND state IN ('queued','running')",
                        [input_id], |row| row.get(0),
                    ).map_err(crate::database_error)
                })?;
                let spoken = fixture.spoken.lock().map_err(|_| "spoken lock")?.clone();
                if saved || done.is_some() || pending_progress != 0
                    || spoken.iter().any(|text| text != "只今お調べします。")
                {
                    return Err(format!("uncommitted answer leaked to speech: {spoken:?}"));
                }
                return Ok(json!({"rejectedWithoutSpeech":true}));
            }
        }
        if done.as_deref() == Some("completed") {
            break;
        }
        if fixture.authentication_failure.load(Ordering::SeqCst) {
            let failed: bool = state.sqlite_readers.read(|connection| {
                connection.query_row("SELECT EXISTS(SELECT 1 FROM task_queue_jobs WHERE job_key=?1 AND kind='user_input' AND state='failed')",
                    [input_id], |row| row.get(0)).map_err(crate::database_error)
            })?;
            if failed { break; }
        }
        if done.as_deref() == Some("failed") || tokio::time::Instant::now() > deadline {
            let jobs = state
                .sqlite_readers
                .read(|connection| task_queue::snapshot(connection, PRIMARY_CONVERSATION_ID))?;
            let details = jobs
                .iter()
                .map(|job| format!("{}:{}:{:?}", job.kind, job.state, job.error))
                .collect::<Vec<_>>();
            return Err(format!(
                "queue did not complete: {done:?}; jobs: {details:?}"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let (jobs, answer) = state.sqlite_readers.read(|connection| {
        let jobs = task_queue::snapshot(connection, PRIMARY_CONVERSATION_ID)?;
        let answer: Option<String> = connection
            .query_row(
                "SELECT content FROM conversation_messages WHERE id=?1",
                [format!("reply_{input_id}")],
                |row| row.get(0),
            )
            .optional()
            .map_err(crate::database_error)?;
        Ok((jobs, answer))
    })?;
    let states = jobs
        .iter()
        .filter(|job| job.key == input_id)
        .map(|job| (job.kind.clone(), job.state.clone()))
        .collect::<Vec<_>>();
    if states.iter().any(|(kind, _)| kind == "ornith_task" || kind == "ornith_result") {
        return Err(format!("new input created a legacy two-model job: {states:?}"));
    }
    if fixture.invalid_reply.load(Ordering::SeqCst) {
        let spoken = fixture.spoken.lock().map_err(|_| "spoken lock")?;
        if answer.as_deref().is_some_and(|answer| answer.contains("<think>"))
            || spoken.iter().any(|s| s.contains("<think>"))
            || !answer.as_deref().is_some_and(|answer| answer.contains("結果を整理する段階で失敗"))
        {
            return Err(format!("invalid model content escaped into a saved or spoken answer: {answer:?}; states={states:?}"));
        }
        return Ok(json!({"rejectedWithoutSpeech":true}));
    }
    if fixture.authentication_failure.load(Ordering::SeqCst) {
        fixture.authentication_failure.store(false, Ordering::SeqCst);
        let next_key = "auth-recovery-greeting";
        conversation_check::queue_runtime::enqueue_text(
            &state,
            next_key.into(),
            "こんにちは".into(),
        )?;
        state.conversation_queue_wake.notify_waiters();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let ready: bool = state.sqlite_readers.read(|c| {
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM conversation_messages WHERE id=?1)",
                    [format!("reply_{next_key}")],
                    |r| r.get(0),
                )
                .map_err(crate::database_error)
            })?;
            if ready {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                return Err("next input after authentication rejection did not recover".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let llm_calls = *fixture.llm_calls.lock().map_err(|_| "LLM count lock")?;
        let creates = fixture
            .calls
            .lock()
            .map_err(|_| "calls lock")?
            .iter()
            .filter(|call| call.as_str() == "POST /v1/agent-connections")
            .count();
        let spoken = fixture.spoken.lock().map_err(|_| "spoken lock")?.clone();
        if llm_calls < 2
            || creates < 2
            || !states
                .iter()
                .any(|(kind, state)| kind == "user_input" && state == "failed")
            || answer.is_some()
            || spoken.iter().any(|speech| speech.contains("Provider authentication failed"))
        {
            return Err(format!("authentication recovery failed: calls={llm_calls}, creates={creates}, states={states:?}"));
        }
        return Ok(json!({"reconnected":true,"llmCalls":llm_calls,"answer":answer}));
    }
    let answer = answer.ok_or("Ornith did not save an answer")?;
    for kind in ["user_input", "speech"] {
        if !states
            .iter()
            .any(|(name, state)| name == kind && state == "completed")
        {
            return Err(format!("{kind} did not complete: {states:?}"));
        }
    }
    let expected_answer = if fixture.fail_after_search.load(Ordering::SeqCst) {
        assert!(answer.contains("結果を整理する段階で失敗"));
        answer.as_str()
    } else {
        "資料では確認済みの事実は42です。"
    };
    let expected_display = if fixture.fail_after_search.load(Ordering::SeqCst) {
        expected_answer.to_string()
    } else {
        format!("{expected_answer}\n\n<!-- saaa:source-links -->\n[出典1: example.invalid](https://example.invalid/report)\n")
    };
    if answer != expected_display {
        return Err(format!("Ornith answer mismatch: {answer}"));
    }
    let spoken = fixture
        .spoken
        .lock()
        .map_err(|_| "spoken lock unavailable")?
        .clone();
    let logged_progress = state.sqlite_readers.read(|connection| {
        let mut statement = connection
            .prepare(
                "SELECT m.content FROM conversation_messages m JOIN task_queue_jobs j
             ON m.id='progress_' || j.id
             WHERE j.scope=?1 AND j.job_key=?2 AND m.role='assistant'
             ORDER BY j.generation",
            )
            .map_err(crate::database_error)?;
        let rows = statement
            .query_map(
                rusqlite::params![PRIMARY_CONVERSATION_ID, input_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(crate::database_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(crate::database_error)
    })?;
    let progress_count = logged_progress.len();
    let expected_progress = state.sqlite_readers.read(|connection| {
        logged_progress.iter().map(|text| crate::tts_dictionary::apply_saved(connection, text)).collect::<Result<Vec<_>, _>>()
    })?;
    if expected_progress
        != spoken
            .iter()
            .take(progress_count)
            .cloned()
            .collect::<Vec<_>>()
    {
        return Err(format!(
            "spoken progress missing from conversation log: {expected_progress:?}"
        ));
    }
    let expected_speech = state.sqlite_readers.read(|connection| {
        crate::tts_dictionary::apply_saved(connection, expected_answer)
    })?;
    if (progress_count > 0 && spoken.first() != expected_progress.first())
        || spoken.get(progress_count..).unwrap_or_default().join("") != expected_speech
    {
        return Err(format!(
            "TTS did not receive the final Ornith answer: {spoken:?}"
        ));
    }
    if spoken.iter().any(|line| line == "もうすこしおまちください。") {
        return Err(format!("retired waiting speech was spoken: {spoken:?}"));
    }
    let pending_progress: i64 = state.sqlite_readers.read(|connection| {
        connection.query_row(
            "SELECT count(*) FROM task_queue_jobs WHERE scope=?1 AND job_key=?2 AND kind='progress_speech' AND state IN ('queued','running')",
            rusqlite::params![PRIMARY_CONVERSATION_ID,input_id], |row| row.get(0),
        ).map_err(crate::database_error)
    })?;
    if pending_progress != 0 {
        return Err("waiting speech was not cancelled after the answer".into());
    }
    let final_spoken = spoken.get(progress_count..).unwrap_or_default().to_vec();
    let searches = fixture
        .searches
        .lock()
        .map_err(|_| "search lock unavailable")?
        .clone();
    if searches != ["fixture fact"] {
        return Err(format!("Ornith tool handoff mismatch: {searches:?}"));
    }
    let calls = fixture
        .calls
        .lock()
        .map_err(|_| "calls lock unavailable")?
        .clone();
    let order = ["/asr/v1/audio/transcriptions", "/llm/v1/chat/completions"];
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.contains("/backchannel/v1/chat/completions"))
            .count(),
        0,
        "conversation must not call the Qwen backchannel"
    );
    let mut cursor = 0;
    for path in order {
        let index = calls
            .iter()
            .enumerate()
            .skip(cursor)
            .find(|(_, call)| call.contains(path))
            .map(|(index, _)| index)
            .ok_or_else(|| format!("Missing provider call {path}: {calls:?}"))?;
        cursor = index + 1;
    }
    if fixture.fail_after_search.load(Ordering::SeqCst) {
        if *fixture
            .llm_calls
            .lock()
            .map_err(|_| "LLM count lock unavailable")?
            != 2
        {
            return Err("Ornith repeated a failed tool follow-up".into());
        }
        let failure_kind: Option<String> = state.sqlite_readers.read(|connection| {
            connection
                .query_row(
                    "SELECT json_extract(attributes_json,'$.kind') FROM audit_events
                 WHERE event_name='conversation-provider-failure' ORDER BY rowid DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(crate::database_error)
        })?;
        return Ok(
            json!({"answer":answer,"spoken":final_spoken,"progressSpoken":logged_progress.first(),"waitingSpoken":null,"searches":searches,
            "jobStates":states,"providerFailureKind":failure_kind}),
        );
    }
    let llm_before = *fixture
        .llm_calls
        .lock()
        .map_err(|_| "LLM count lock unavailable")?;
    let greeting_id = "queue-e2e-greeting";
    conversation_check::queue_runtime::enqueue_text(
        &state,
        greeting_id.into(),
        "こんにちは".into(),
    )?;
    state.conversation_queue_wake.notify_waiters();
    let greeting_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let greeting_state = state.sqlite_readers.read(|connection| {
            connection.query_row(
                "SELECT state FROM task_queue_jobs WHERE scope=?1 AND kind='speech' AND job_key=?2",
                rusqlite::params![PRIMARY_CONVERSATION_ID,greeting_id], |row| row.get::<_,String>(0),
            ).optional().map_err(crate::database_error)
        })?;
        if greeting_state.as_deref() == Some("completed") {
            break;
        }
        if greeting_state.as_deref() == Some("failed")
            || tokio::time::Instant::now() > greeting_deadline
        {
            return Err(format!(
                "quick greeting did not complete: {greeting_state:?}"
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let greeting_jobs = state
        .sqlite_readers
        .read(|connection| task_queue::snapshot(connection, PRIMARY_CONVERSATION_ID))?;
    if !greeting_jobs
        .iter()
        .any(|job| job.key == greeting_id && job.kind == "user_input" && job.state == "completed")
    {
        return Err("greeting did not reach Ornith".into());
    }
    if *fixture
        .llm_calls
        .lock()
        .map_err(|_| "LLM count lock unavailable")?
        <= llm_before
    {
        return Err("greeting did not call Ornith".into());
    }
    let spoken_after = fixture
        .spoken
        .lock()
        .map_err(|_| "spoken lock unavailable")?
        .clone();
    if spoken_after.last().map(String::as_str) != Some("こんにちは。") {
        return Err(format!("quick greeting speech mismatch: {spoken_after:?}"));
    }
    cancellation::verify(&state, fixture).await?;
    Ok(
        json!({"cancellationVerified":true,"transcript":transcript,"answer":answer,"spoken":final_spoken,"progressSpoken":logged_progress.first(),"waitingSpoken":null,"searches":searches,
        "jobStates":states,"providerCalls":calls,"quickGreeting":spoken_after.last(),
        "speechBeforeDone":fixture.speech_before_done.load(Ordering::SeqCst)}),
    )
}

pub async fn run_live_web_retrieval() -> Result<Value, String> {
    crate::runtime::web_fetch::live_retrieval().await
}
