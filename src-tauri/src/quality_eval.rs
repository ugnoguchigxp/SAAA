use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::Instant,
};

use crate::{
    memory, persistence, providers, runtime, situation, voice, AppState, RunCancellation,
    StartTurnInput, PRIMARY_CONVERSATION_ID,
};

// The fixture follows this evaluation future, including across thread switches.
// Concurrent runs never share process environment or inherit another run's fixture.
tokio::task_local! {
    pub(crate) static TOOL_FIXTURE: String;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct QualityRequest {
    base_url: String,
    api_key: String,
    model: String,
    input: String,
    input_origin: String,
    timeout_ms: u64,
    tool_mode: String,
    tool_result: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct QualityResponse {
    content: String,
    latency_ms: u128,
    runtime_path: &'static str,
}

pub async fn run_json(input: &str) -> Result<String, String> {
    let request: QualityRequest = serde_json::from_str(input)
        .map_err(|error| format!("Invalid quality runtime request: {error}"))?;
    validate_request(&request)?;
    let fixture = match request.tool_mode.as_str() {
        "none" => runtime::agent_tools::tool_error_content(
            "fixture-tool-not-expected",
            "No external tool fixture is available for this scenario.",
        ),
        "success" => request.tool_result.clone().unwrap_or_default(),
        "failure" => runtime::agent_tools::tool_error_content(
            "fixture-network-failure",
            request
                .tool_result
                .as_deref()
                .unwrap_or("Deterministic network failure."),
        ),
        _ => return Err("Invalid quality tool mode".to_string()),
    };

    let state = quality_state(&request)?;
    let run_id = crate::new_id("quality-run");
    let turn = StartTurnInput {
        run_id,
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: request.input,
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: request.input_origin,
        presentation_mode: "visual-and-spoken".to_string(),
    };
    let channel = tauri::ipc::Channel::new(|_| Ok(()));
    let started = Instant::now();
    TOOL_FIXTURE
        .scope(
            fixture,
            runtime::turns::execute_turn(
                &state,
                &turn,
                &channel,
                Arc::new(RunCancellation::default()),
                None,
            ),
        )
        .await
        .map_err(|error| error.message)?;
    let content = state.sqlite_readers.read(|connection| {
        connection
            .query_row(
                "SELECT content FROM conversation_messages
                 WHERE conversation_id=?1 AND role='assistant'
                 ORDER BY rowid DESC LIMIT 1",
                [PRIMARY_CONVERSATION_ID],
                |row| row.get::<_, String>(0),
            )
            .map_err(crate::database_error)
    })?;
    serde_json::to_string(&QualityResponse {
        content,
        latency_ms: started.elapsed().as_millis(),
        runtime_path: "execute_turn/conversation.respond",
    })
    .map_err(|error| format!("Could not encode quality runtime response: {error}"))
}

fn validate_request(request: &QualityRequest) -> Result<(), String> {
    let endpoint = url::Url::parse(&request.base_url)
        .map_err(|_| "Quality endpoint is invalid".to_string())?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host_str().is_none()
        || request.api_key.is_empty()
        || request.api_key.len() > 16_384
        || request.model.trim().is_empty()
        || request.model.chars().count() > 256
        || request.input.trim().is_empty()
        || request.input.chars().count() > 32_000
        || !matches!(request.input_origin.as_str(), "text" | "voice")
        || !(1_000..=120_000).contains(&request.timeout_ms)
    {
        return Err("Quality runtime request violates its bounded contract".to_string());
    }
    Ok(())
}

fn quality_state(request: &QualityRequest) -> Result<AppState, String> {
    let mut connection = Connection::open_in_memory().map_err(crate::database_error)?;
    persistence::schema::initialize_database(&connection).map_err(crate::database_error)?;
    let endpoint = url::Url::parse(&request.base_url)
        .map_err(|_| "Quality endpoint is invalid".to_string())?;
    let location = if endpoint.scheme() == "https" {
        "cloud"
    } else {
        "local"
    };
    let mut documents = persistence::settings::default_settings_documents()
        .into_iter()
        .map(
            |(namespace, key, schema_version, value_json)| crate::SaveSettingsDocumentInput {
                namespace: namespace.to_string(),
                key: key.to_string(),
                schema_version,
                value_json,
            },
        )
        .collect::<Vec<_>>();
    for document in &mut documents {
        match (document.namespace.as_str(), document.key.as_str()) {
            ("providers.model", "default") => {
                document.value_json = json!({
                    "harness": { "address": "http://localhost:9810" },
                    "providers": [{
                        "kind": "openai-compatible",
                        "id": "quality-eval",
                        "enabled": true,
                        "label": "Quality Eval",
                        "location": location,
                        "endpoint": request.base_url,
                        "model": request.model,
                        "authentication": "none"
                    }],
                    "reasoningEffort": providers::DEFAULT_CONVERSATION_REASONING_EFFORT
                });
            }
            ("providers.agent", "codex-sdk") => {
                document.value_json["agentName"] = json!("SAAA Eval Agent");
                document.value_json["userName"] = json!("");
            }
            ("routing.roles", "default") => {
                document.value_json["enabled"] = json!(false);
            }
            ("routing.tasks", "default") => {
                document.value_json["conversationRespond"] = json!({
                    "source": "provider",
                    "primaryProviderId": "quality-eval",
                    "fallbackProviderIds": [],
                    "timeoutMs": request.timeout_ms
                });
                document.value_json["voiceSpeak"] = json!({
                    "source": "harness",
                    "providerId": null,
                    "timeoutMs": 30000
                });
            }
            _ => {}
        }
    }
    persistence::save_settings_documents_to_connection(&mut connection, &documents)?;
    let situation_settings = situation::repository::load_settings(&connection)?;
    let sqlite_writer = Arc::new(persistence::SqliteWriter::from_connection(connection));
    let sqlite_readers = persistence::SqliteReaders::serialized(sqlite_writer.clone());
    let generated_capabilities = Arc::new(
        crate::generated_capabilities::service::CapabilityService::build(
            sqlite_writer.clone(),
            &PathBuf::new(),
            PathBuf::new(),
            None,
        ),
    );
    let tool_selection = Arc::new(crate::tool_selection::build_service(
        sqlite_writer.clone(),
        &crate::tool_selection::ToolSelectionConfig::direct(),
        Some(generated_capabilities.clone()),
    ));
    Ok(AppState {
        sqlite_writer,
        sqlite_readers,
        data_directory: PathBuf::new(),
        context_still_recall: memory::context_still_recall::ContextStillRecallClient::disabled(),
        context_still_search: memory::context_still_search::ContextStillSearchClient::disabled(),
        active_runs: Mutex::new(HashMap::new()),
        provider_probes: Mutex::new(HashMap::new()),
        interaction_policy: Mutex::new(()),
        shutdown_started: AtomicBool::new(false),
        audio_uploads: voice::audio_upload::AudioUploadStore::default(),
        streaming_tts: voice::streaming_tts::runtime::StreamingSpeechRuntime::default(),
        voice_behavior: crate::voice_behavior::VoiceBehaviorRuntime::default(),
        situation: Arc::new(situation::SituationRuntime::new(situation_settings, None)?),
        voice_profile: Arc::new(voice::profile::VoiceProfileRuntime::unavailable_for_tests(
            PathBuf::new(),
        )),
        voice_asr: voice::streaming_asr::AsrSessionManager::default(),
        generated_capabilities,
        generation: None,
        generated_tools: crate::generated_capabilities::publication::GeneratedToolsConfig::disabled(
        ),
        tool_selection,
        mcp_server: std::sync::Mutex::new(None),
        schedule: Arc::new(crate::schedule::Handle::default()),
        steward_wake: crate::steward::pump::Wake::default(),
        artifact_preview: crate::artifact_preview::PreviewRuntime::default(),
        reachability: std::sync::Arc::new(
            crate::providers::reachability::ReachabilityState::default(),
        ),
        reachability_kick: std::sync::Arc::new(tokio::sync::Notify::new()),
        diagnosis: std::sync::Arc::new(crate::diagnosis::store::DiagnosisStore::new()),
        context_segments_enabled: std::env::var("SAAA_CONTEXT_SEGMENTS").ok().as_deref()
            == Some("1"),
        wire_prefixes: std::sync::Mutex::new(std::collections::VecDeque::new()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    async fn harness_runs_the_persisted_conversation_runtime_path() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let start: serde_json::Value = loop {
                let mut buffer = [0; 8192];
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..end]);
                    assert!(header.starts_with("POST /v1/chat/completions HTTP/1.1"));
                    let length = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|v| v.parse::<usize>().ok())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        break serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
                    }
                }
            };
            assert_eq!(start["stream"], true);
            assert!(start["tools"].is_array());
            assert!(start["messages"].to_string().contains("SAAA Eval Agent"));
            let body = format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"model":"fixture-model","choices":[{"index":0,"delta":{"content":"runtime answer"},"finish_reason":"stop"}]})
            );
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let input = json!({
            "baseUrl": format!("http://{address}/v1"),
            "apiKey": "fixture-key",
            "model": "fixture-model",
            "input": "hello",
            "inputOrigin": "text",
            "timeoutMs": 5_000,
            "toolMode": "none",
            "toolResult": null
        });
        let response = run_json(&input.to_string())
            .await
            .expect("runtime succeeds");
        server.await.expect("fixture joins");
        let response: serde_json::Value = serde_json::from_str(&response).expect("response JSON");
        assert_eq!(response["content"], "runtime answer");
        assert_eq!(response["runtimePath"], "execute_turn/conversation.respond");
    }

    #[tokio::test]
    async fn concurrent_evaluations_keep_their_own_fixture_and_release_it() {
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let run = |value: &'static str| {
            let barrier = barrier.clone();
            tokio::spawn(TOOL_FIXTURE.scope(value.to_string(), async move {
                barrier.wait().await;
                tokio::task::yield_now().await;
                assert_eq!(TOOL_FIXTURE.with(Clone::clone), value);
            }))
        };
        let (left, right) = tokio::join!(run("first-secret-fixture"), run("second-secret-fixture"));
        left.unwrap();
        right.unwrap();
        assert!(TOOL_FIXTURE.try_with(Clone::clone).is_err());
    }
}
