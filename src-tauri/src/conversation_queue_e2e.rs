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

#[derive(Default)]
struct Fixture {
    calls: Mutex<Vec<String>>,
    spoken: Mutex<Vec<String>>,
    searches: Mutex<Vec<String>>,
    llm_calls: Mutex<usize>,
    speech_before_done: AtomicBool,
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
    true
}
pub(crate) fn web_search(query: &str) -> Option<String> {
    let fixture = fixture()?;
    fixture
        .searches
        .lock()
        .expect("fixture search lock")
        .push(query.into());
    Some(json!({"results":[{"title":"Fixture report","url":"https://example.invalid/report","summary":"確認済みの事実は42です。"}]}).to_string())
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
    let providers = ["asr", "backchannel", "llm", "tts", "embedding"]
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
    let providers = ["asr", "backchannel", "llm", "tts", "embedding"]
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
            "providers":[declaration("asr"),declaration("backchannel"),declaration("llm"),declaration("tts"),declaration("embedding")],"services":[]}]})).into_response();
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
            let mut count = fixture.llm_calls.lock().expect("LLM fixture count");
            let result = if *count == 0 {
                json!({"action":"web_search","query":"fixture fact"}).to_string()
            } else {
                assert!(
                    serialized.contains("確認済みの事実は42"),
                    "Ornith must read tool result"
                );
                json!({"action":"answer","content":"資料では確認済みの事実は42です。"}).to_string()
            };
            *count += 1;
            result
        } else if serialized.contains("会話の入口") {
            json!({"route":"think","reply":null}).to_string()
        } else if serialized.contains("挨拶に自然な日本語") {
            "こんにちは。".into()
        } else {
            assert!(
                serialized.contains("確認済みの事実は42"),
                "Qwen must read Ornith result"
            );
            "調査結果は42です。".into()
        };
        if body["stream"] == true {
            let model = body["model"].as_str().unwrap_or("fixture-qwen");
            let event = json!({"model":model,"choices":[{"index":0,"delta":{"content":content},"finish_reason":"stop"}]});
            let first = format!("data: {event}\n\n");
            let before_done = fixture.clone();
            let chunks = futures_util::stream::once(async move {
                Ok::<_, std::io::Error>(axum::body::Bytes::from(first))
            })
            .chain(futures_util::stream::once(async move {
                let first_speech = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    loop {
                        if !before_done
                            .spoken
                            .lock()
                            .expect("fixture spoken lock")
                            .is_empty()
                        {
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                })
                .await
                .is_ok();
                before_done
                    .speech_before_done
                    .store(first_speech, Ordering::SeqCst);
                Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"data: [DONE]\n\n"))
            }));
            return (
                [(header::CONTENT_TYPE, "text/event-stream")],
                axum::body::Body::from_stream(chunks),
            )
                .into_response();
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

pub async fn run() -> Result<Value, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| error.to_string())?;
    let base = format!(
        "http://{}",
        listener.local_addr().map_err(|error| error.to_string())?
    );
    let fixture = Arc::new(Fixture::default());
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
    let input_id = "queue-e2e-main";
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
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let done = state.sqlite_readers.read(|connection| {
            let state: Option<String> = connection.query_row(
                "SELECT state FROM task_queue_jobs WHERE scope=?1 AND kind='speech' AND job_key=?2",
                rusqlite::params![PRIMARY_CONVERSATION_ID,input_id], |row| row.get(0),
            ).optional().map_err(crate::database_error)?;
            Ok(state)
        })?;
        if done.as_deref() == Some("completed") {
            break;
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
        let answer: String = connection
            .query_row(
                "SELECT content FROM conversation_messages WHERE id=?1",
                [format!("reply_{input_id}")],
                |row| row.get(0),
            )
            .map_err(crate::database_error)?;
        Ok((jobs, answer))
    })?;
    let states = jobs
        .iter()
        .filter(|job| job.key == input_id)
        .map(|job| (job.kind.clone(), job.state.clone()))
        .collect::<Vec<_>>();
    for kind in ["user_input", "ornith_task", "ornith_result", "speech"] {
        if !states
            .iter()
            .any(|(name, state)| name == kind && state == "completed")
        {
            return Err(format!("{kind} did not complete: {states:?}"));
        }
    }
    if answer != "調査結果は42です。" {
        return Err(format!("Qwen answer mismatch: {answer}"));
    }
    let spoken = fixture
        .spoken
        .lock()
        .map_err(|_| "spoken lock unavailable")?
        .clone();
    if spoken.join("") != answer {
        return Err(format!("TTS did not receive only Qwen text: {spoken:?}"));
    }
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
    let order = [
        "/asr/v1/audio/transcriptions",
        "/backchannel/v1/chat/completions",
        "/llm/v1/chat/completions",
        "/backchannel/v1/chat/completions",
    ];
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
    if greeting_jobs
        .iter()
        .any(|job| job.key == greeting_id && job.kind == "ornith_task")
    {
        return Err("quick greeting unexpectedly reached Ornith".into());
    }
    if *fixture
        .llm_calls
        .lock()
        .map_err(|_| "LLM count lock unavailable")?
        != llm_before
    {
        return Err("quick greeting called Ornith".into());
    }
    let spoken_after = fixture
        .spoken
        .lock()
        .map_err(|_| "spoken lock unavailable")?
        .clone();
    if spoken_after != [answer.as_str(), "こんにちは。"] {
        return Err(format!("quick greeting speech mismatch: {spoken_after:?}"));
    }
    cancellation::verify(&state, fixture).await?;
    Ok(
        json!({"cancellationVerified":true,"transcript":transcript,"answer":answer,"spoken":spoken,"searches":searches,
        "jobStates":states,"providerCalls":calls,"quickGreeting":spoken_after[1],
        "speechBeforeDone":fixture.speech_before_done.load(Ordering::SeqCst)}),
    )
}
