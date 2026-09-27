#![cfg(feature = "provider-unit-test-harness")]
use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, Method, StatusCode},
    response::{IntoResponse, Response},
    Json, Router,
};
use base64::Engine;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

struct Fixture {
    base: String,
    calls: Mutex<Vec<String>>,
    scopes: Mutex<Vec<Option<Vec<String>>>>,
}

const NAMES: [&str; 5] = ["asr", "tts", "backchannel", "llm", "embedding"];

fn protocol(name: &str) -> &'static str {
    match name {
        "asr" => "openai.audio-transcriptions.v1",
        "tts" => "openai.audio-speech.v1",
        "embedding" => "larm.embedding.v1",
        _ => "openai.chat-completions.v1",
    }
}

fn endpoint(name: &str) -> &'static str {
    match name {
        "asr" => "/v1/audio/transcriptions",
        "tts" => "/v1/audio/speech",
        "embedding" => "/v1/embed",
        _ => "/v1/chat/completions",
    }
}

fn model(name: &str) -> &'static str {
    match name {
        "asr" => "qwen3-asr-1.7b",
        "tts" => "voicevox-core",
        "backchannel" => "qwen3.5-2b-fast-response",
        "llm" => "ornith-1.5-35b",
        _ => "multilingual-e5-small",
    }
}

fn declaration(name: &str) -> Value {
    let capability = match name {
        "asr" => "speech.stt",
        "tts" => "speech.tts",
        "backchannel" => "llm.backchannel.classifier",
        "llm" => "llm.general",
        _ => "embedding",
    };
    let mut value = json!({
        "name": name, "capability": capability, "protocol": protocol(name),
        "endpoint": endpoint(name), "model": model(name)
    });
    if matches!(name, "llm" | "backchannel") {
        value["contextWindow"] =
            json!({"maxTokens":65536,"outputReserveTokens":4096,"safetyMarginTokens":1976});
    }
    value
}

fn selected_names(fixture: &Fixture) -> Vec<String> {
    fixture
        .scopes
        .lock()
        .unwrap()
        .last()
        .cloned()
        .flatten()
        .unwrap_or_else(|| NAMES.iter().map(|name| (*name).to_string()).collect())
}

fn state_value(fixture: &Fixture) -> Value {
    let providers = selected_names(fixture)
        .iter()
        .map(|name| {
            let mut value = declaration(name);
            value["readiness"] = json!("ready");
            value["claimable"] = json!(true);
            value
        })
        .collect::<Vec<_>>();
    json!({
        "id":"session-1", "profile":"SAAA", "agentProfile":"saaa-conversation-ornith15",
        "providers":providers, "services":[], "status":"ready",
        "expiresAt":(chrono::Utc::now()+chrono::Duration::seconds(900)).to_rfc3339()
    })
}

fn claim_value(fixture: &Fixture) -> Value {
    let providers = selected_names(fixture)
        .iter()
        .map(|name| {
            let name = name.as_str();
            let base = format!("{}/{name}/v1", fixture.base);
            let mut value = json!({
                "name":name, "protocol":protocol(name), "baseUrl":base,
                "model":model(name),
                "configuration":{"fields":if name == "embedding" {
                    json!({"daemonURL":base,"model":model(name),"dimension":384})
                } else {
                    json!({"baseURL":base,"model":model(name)})
                }},
                "credential":{"token":format!("token-{name}")},
                "health":{"url":format!("{}/{name}/health",fixture.base),"maxAgeMs":10000},
                "embeddingSpace":if name == "embedding" { json!({"dimension":384}) } else { Value::Null }
            });
            if matches!(name, "llm" | "backchannel") {
                value["contextWindow"] =
                    json!({"maxTokens":65536,"outputReserveTokens":4096,"safetyMarginTokens":1976});
            }
            value
        })
        .collect::<Vec<_>>();
    let mut value = state_value(fixture);
    value["allocationId"] = json!("allocation-1");
    value["providers"] = json!(providers);
    value
}

async fn handle(State(fixture): State<Arc<Fixture>>, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let method = request.method().clone();
    fixture
        .calls
        .lock()
        .unwrap()
        .push(format!("{method} {path}"));
    if path == "/v3/agent-profiles" {
        return Json(json!({
            "contractVersion":"agent-connection.v3", "catalogRevision":"rev-fixture",
            "requestedProfile":"SAAA",
            "profiles":[{"id":"saaa-conversation-ornith15",
                "providers":NAMES.iter().map(|name| declaration(name)).collect::<Vec<_>>(),
                "services":[]}]
        }))
        .into_response();
    }
    if path.starts_with("/v1/agent-connections") {
        assert_eq!(
            request.headers()[header::AUTHORIZATION],
            "Bearer control-token"
        );
        if method == Method::DELETE {
            return StatusCode::NO_CONTENT.into_response();
        }
        if path.ends_with("/claim") {
            let bytes = to_bytes(request.into_body(), 1_000_000).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            let expected_format = if selected_names(&fixture) == ["embedding"] {
                "larm-embedding-provider-v1"
            } else {
                "openai-provider-v1"
            };
            assert_eq!(body["format"], expected_format);
            return Json(claim_value(&fixture)).into_response();
        }
        if method == Method::POST {
            let bytes = to_bytes(request.into_body(), 1_000_000).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            let scope = body.get("providers").map(|providers| {
                providers
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|name| name.as_str().unwrap().to_string())
                    .collect()
            });
            fixture.scopes.lock().unwrap().push(scope);
            return (StatusCode::CREATED, Json(state_value(&fixture))).into_response();
        }
        return Json(state_value(&fixture)).into_response();
    }
    if path == "/v1/audio/speech" {
        assert_eq!(
            request.headers()[header::AUTHORIZATION],
            "Bearer control-token"
        );
        let bytes = to_bytes(request.into_body(), 1_000_000).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["input"], "こんにちは");
        assert_eq!(body["voice"], "Kasukabe_Tsumugi");
        return (
            [(header::CONTENT_TYPE, "audio/wav")],
            Body::from(b"RIFFfixture-wave-data".to_vec()),
        )
            .into_response();
    }
    let name = path.split('/').nth(1).unwrap_or_default();
    assert_eq!(
        request.headers()[header::AUTHORIZATION],
        format!("Bearer token-{name}")
    );
    if path.ends_with("/health") {
        return Json(json!({
            "ready":true, "acceptingRequests":true,
            "capacity":{"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":1,
                "queueDepth":0,"queueTimeoutMs":1000,"retryAfterMs":0,"completionGuaranteed":false},
            "probe":{"validated":true,"protocol":protocol(name)}
        }))
        .into_response();
    }
    if path == "/tts/v1/audio/voices" {
        return Json(json!({
            "default_voice":"fixture-voice",
            "voices":[{"id":"fixture-voice","display_name":"Fixture Voice"}]
        }))
        .into_response();
    }
    let bytes = to_bytes(request.into_body(), 1_000_000).await.unwrap();
    match (name, path.as_str()) {
        ("asr", "/asr/v1/audio/transcriptions") => {
            assert!(bytes.windows(4).any(|part| part == b"RIFF"));
            Json(json!({"text":"音声のテスト結果","language":"ja"})).into_response()
        }
        ("tts", "/tts/v1/audio/speech") => {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["input"], "こんにちは");
            assert_eq!(body["voice"], "fixture-voice");
            (
                [(header::CONTENT_TYPE, "audio/wav")],
                Body::from(b"RIFFfixture-wave-data".to_vec()),
            )
                .into_response()
        }
        ("llm" | "backchannel", _) => {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["model"], model(name));
            assert_eq!(body["messages"][0]["content"], "応答してください");
            Json(
                json!({"choices":[{"message":{"content":format!("{name}-answer")},
                "finish_reason":"stop"}]}),
            )
            .into_response()
        }
        ("embedding", "/embedding/v1/embed") => {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["texts"], json!(["埋め込みテスト"]));
            Json(json!({"embeddings":[vec![0.25_f32;384]]})).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

#[tokio::test]
async fn asr_sends_wav_after_claim_and_releases_after_transcription() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Arc::new(Fixture {
        base: format!("http://{}", listener.local_addr().unwrap()),
        calls: Mutex::new(Vec::new()),
        scopes: Mutex::new(Vec::new()),
    });
    let router = Router::new().fallback(handle).with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let samples = vec![0.25; 3_200];
    let result = saaa_lib::runtime::provider_unit_test::run_fixture_provider_unit_test(
        &fixture.base,
        "SAAA",
        "",
        "control-token",
        "asr",
        "",
        Some(&samples),
    )
    .await
    .unwrap();
    assert_eq!(result["capability"], "asr");
    assert_eq!(result["model"], model("asr"));
    assert_eq!(result["output"], "音声のテスト結果\n\n検出言語: ja");
    let calls = fixture.calls.lock().unwrap();
    let position = |call: &str| calls.iter().position(|value| value == call).unwrap();
    assert!(
        position("POST /v1/agent-connections/session-1/claim")
            < position("POST /asr/v1/audio/transcriptions")
    );
    assert!(
        position("POST /asr/v1/audio/transcriptions")
            < position("DELETE /v1/agent-connections/session-1")
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.starts_with("POST /")
                && !call.starts_with("POST /v1/agent-connections"))
            .count(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn text_provider_tests_use_separate_subsets_and_reuse_matching_session() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Arc::new(Fixture {
        base: format!("http://{}", listener.local_addr().unwrap()),
        calls: Mutex::new(Vec::new()),
        scopes: Mutex::new(Vec::new()),
    });
    let router = Router::new().fallback(handle).with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for name in ["backchannel", "llm", "llm", "embedding"] {
        let text = if name == "embedding" {
            "埋め込みテスト"
        } else {
            "応答してください"
        };
        let result = saaa_lib::runtime::provider_unit_test::run_fixture_provider_unit_test_cached(
            &fixture.base,
            "SAAA",
            "control-token",
            name,
            text,
        )
        .await
        .unwrap();
        if name == "embedding" {
            assert!(result["output"].as_str().unwrap().contains("次元数: 384"));
        } else {
            assert_eq!(result["output"], format!("{name}-answer"));
        }
    }
    let calls = fixture.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|call| *call == "POST /v1/agent-connections")
            .count(),
        3
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| *call == "POST /v1/agent-connections/session-1/claim")
            .count(),
        3
    );
    assert_eq!(
        *fixture.scopes.lock().unwrap(),
        vec![
            Some(vec!["backchannel".into()]),
            Some(vec!["llm".into()]),
            Some(vec!["embedding".into()]),
        ]
    );
    assert!(calls.contains(&"POST /backchannel/v1/chat/completions".to_string()));
    assert!(calls.contains(&"POST /llm/v1/chat/completions".to_string()));
    assert!(calls.contains(&"POST /embedding/v1/embed".to_string()));
    drop(calls);
    saaa_lib::runtime::provider_unit_test::release_cached_provider_unit_test_session()
        .await
        .unwrap();
    server.abort();
}

#[tokio::test]
async fn four_provider_requests_reach_only_the_selected_claimed_endpoint() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fixture = Arc::new(Fixture {
        base: format!("http://{}", listener.local_addr().unwrap()),
        calls: Mutex::new(Vec::new()),
        scopes: Mutex::new(Vec::new()),
    });
    let router = Router::new().fallback(handle).with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for (name, text) in [
        ("tts", "こんにちは"),
        ("backchannel", "応答してください"),
        ("llm", "応答してください"),
        ("embedding", "埋め込みテスト"),
    ] {
        let before = fixture.calls.lock().unwrap().len();
        let result = saaa_lib::runtime::provider_unit_test::run_fixture_provider_unit_test(
            &fixture.base,
            "SAAA",
            "",
            "control-token",
            name,
            text,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result["capability"], name);
        assert_eq!(result["model"], model(name));
        assert!(result["latencyMs"].as_u64().unwrap() < 120_000);
        match name {
            "tts" => {
                let audio = base64::engine::general_purpose::STANDARD
                    .decode(result["audioBase64"].as_str().unwrap())
                    .unwrap();
                assert!(audio.starts_with(b"RIFF"));
            }
            "embedding" => assert!(result["output"].as_str().unwrap().contains("次元数: 384")),
            _ => assert_eq!(result["output"], format!("{name}-answer")),
        }
        let calls = fixture.calls.lock().unwrap();
        let current = &calls[before..];
        if matches!(name, "backchannel" | "llm" | "embedding") {
            assert_eq!(
                fixture.scopes.lock().unwrap().last().unwrap(),
                &Some(vec![name.into()])
            );
        }
        if name == "tts" {
            assert!(!current
                .iter()
                .any(|call| call.contains("/agent-connections")));
        }
        assert!(
            current.contains(&if name == "tts" {
                "POST /v1/audio/speech".to_string()
            } else {
                format!("POST /{name}{}", endpoint(name))
            }),
            "unexpected requests for {name}: {current:?}"
        );
        assert_eq!(
            current
                .iter()
                .filter(|call| call.contains("/v1/chat/completions")
                    || call.contains("/v1/audio/transcriptions")
                    || call.contains("/v1/audio/speech")
                    || call.contains("/v1/embed"))
                .count(),
            1
        );
        if name != "tts" {
            assert!(current.contains(&"DELETE /v1/agent-connections/session-1".to_string()));
        }
    }
    server.abort();
}

#[tokio::test]
#[ignore = "requires a live LARM and LARM_API_TOKEN"]
async fn tts_completes_against_live_larm_without_session() {
    let address = std::env::var("SAAA_LARM_URL").expect("SAAA_LARM_URL");
    let credential = std::env::var("LARM_API_TOKEN").expect("LARM_API_TOKEN");
    let result = saaa_lib::runtime::provider_unit_test::run_fixture_provider_unit_test(
        &address,
        "SAAA",
        "",
        &credential,
        "tts",
        "こんにちは",
        None,
    )
    .await
    .unwrap();
    assert_eq!(result["model"], "voicevox-core");
    let audio = base64::engine::general_purpose::STANDARD
        .decode(result["audioBase64"].as_str().unwrap())
        .unwrap();
    assert!(audio.starts_with(b"RIFF"));
}

#[tokio::test]
#[ignore = "requires a live LARM and LARM_API_TOKEN"]
async fn embedding_completes_against_live_larm() {
    let address = std::env::var("SAAA_LARM_URL").expect("SAAA_LARM_URL");
    let credential = std::env::var("LARM_API_TOKEN").expect("LARM_API_TOKEN");
    let result = saaa_lib::runtime::provider_unit_test::run_fixture_provider_unit_test_cached(
        &address,
        "SAAA",
        &credential,
        "embedding",
        "これは埋め込みのテストです。",
    )
    .await;
    saaa_lib::runtime::provider_unit_test::release_cached_provider_unit_test_session()
        .await
        .unwrap();
    let result = result.unwrap();
    assert_eq!(result["capability"], "embedding");
    assert!(result["output"].as_str().unwrap().contains("次元数:"));
    eprintln!("embedding response: {}", result["output"]);
}

#[tokio::test]
#[ignore = "requires a live LARM and LARM_API_TOKEN"]
async fn text_provider_default_prompts_answer_against_live_larm() {
    let address = std::env::var("SAAA_LARM_URL").expect("SAAA_LARM_URL");
    let credential = std::env::var("LARM_API_TOKEN").expect("LARM_API_TOKEN");
    let mut responses = Vec::new();
    for name in ["backchannel", "llm"] {
        let result = saaa_lib::runtime::provider_unit_test::run_fixture_provider_unit_test_cached(
            &address,
            "SAAA",
            &credential,
            name,
            "短く自己紹介してください。",
        )
        .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                let _ = saaa_lib::runtime::provider_unit_test::release_cached_provider_unit_test_session().await;
                panic!("{name} failed: {error}");
            }
        };
        responses.push((name, result));
    }
    saaa_lib::runtime::provider_unit_test::release_cached_provider_unit_test_session()
        .await
        .unwrap();
    for (name, result) in responses {
        let output = result["output"].as_str().unwrap();
        assert_eq!(result["model"], model(name));
        eprintln!(
            "{name} response ({} bytes, {} ms): {}",
            output.len(),
            result["latencyMs"],
            output.chars().take(200).collect::<String>()
        );
        assert!(!output.trim().is_empty());
    }
}

#[tokio::test]
#[ignore = "requires a live LARM and LARM_API_TOKEN"]
async fn four_provider_requests_complete_against_live_larm() {
    let address = std::env::var("SAAA_LARM_URL").expect("SAAA_LARM_URL");
    let credential = std::env::var("LARM_API_TOKEN").expect("LARM_API_TOKEN");
    for (name, text) in [
        ("tts", "こんにちは"),
        ("backchannel", "短く挨拶してください。"),
        ("llm", "短く挨拶してください。"),
        ("embedding", "埋め込みテスト"),
    ] {
        let result = saaa_lib::runtime::provider_unit_test::run_fixture_provider_unit_test(
            &address,
            "SAAA",
            "",
            &credential,
            name,
            text,
            None,
        )
        .await
        .unwrap_or_else(|error| panic!("{name} failed: {error}"));
        assert_eq!(result["capability"], name);
        assert!(!result["output"].as_str().unwrap_or_default().is_empty());
        if name == "tts" {
            let audio = base64::engine::general_purpose::STANDARD
                .decode(result["audioBase64"].as_str().unwrap())
                .unwrap();
            assert!(audio.starts_with(b"RIFF"));
        }
    }
}
