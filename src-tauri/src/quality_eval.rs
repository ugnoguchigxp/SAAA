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
    meeting, memory, persistence, providers, runtime, situation, voice, AppState, RunCancellation,
    StartTurnInput, PRIMARY_CONVERSATION_ID,
};

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
    std::env::set_var("SAAA_PROVIDER_QUALITY_EVAL_API_KEY", &request.api_key);
    match request.tool_mode.as_str() {
        "none" => std::env::set_var(
            "SAAA_QUALITY_TOOL_FIXTURE",
            runtime::agent_tools::tool_error_content(
                "fixture-tool-not-expected",
                "No external tool fixture is available for this scenario.",
            ),
        ),
        "success" => std::env::set_var(
            "SAAA_QUALITY_TOOL_FIXTURE",
            request.tool_result.as_deref().unwrap_or_default(),
        ),
        "failure" => std::env::set_var(
            "SAAA_QUALITY_TOOL_FIXTURE",
            runtime::agent_tools::tool_error_content(
                "fixture-network-failure",
                request
                    .tool_result
                    .as_deref()
                    .unwrap_or("Deterministic network failure."),
            ),
        ),
        _ => return Err("Invalid quality tool mode".to_string()),
    }

    let state = quality_state(&request)?;
    let run_id = crate::new_id("quality-run");
    let turn = StartTurnInput {
        run_id,
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: request.input,
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        input_origin: request.input_origin,
        presentation_mode: "visual-and-spoken".to_string(),
    };
    let channel = tauri::ipc::Channel::new(|_| Ok(()));
    let started = Instant::now();
    runtime::turns::execute_turn(
        &state,
        &turn,
        &channel,
        Arc::new(RunCancellation::default()),
        None,
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
    Ok(AppState {
        sqlite_writer,
        sqlite_readers,
        data_directory: PathBuf::new(),
        context_still_recall: memory::context_still_recall::ContextStillRecallClient::disabled(),
        active_runs: Mutex::new(HashMap::new()),
        provider_probes: Mutex::new(HashMap::new()),
        interaction_policy: Mutex::new(()),
        shutdown_started: AtomicBool::new(false),
        larm_gate: providers::larm::LarmRuntimeGate::Disabled,
        network_asr: voice::network_asr::NetworkAsrRuntime::new()?,
        audio_uploads: voice::audio_upload::AudioUploadStore::default(),
        tts_process: Mutex::new(None),
        streaming_tts: voice::streaming_tts::runtime::StreamingSpeechRuntime::default(),
        voice_behavior: crate::voice_behavior::VoiceBehaviorRuntime::default(),
        situation: Arc::new(situation::SituationRuntime::new(situation_settings, None)?),
        meeting: Arc::new(meeting::MeetingRuntime::new()),
        voice_profile: Arc::new(voice::profile::VoiceProfileRuntime::unavailable_for_tests(
            PathBuf::new(),
        )),
        voice_asr: voice::streaming_asr::AsrSessionManager::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use sha2::{Digest, Sha256};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{
            handshake::server::{Request, Response},
            http::{header, HeaderValue},
            Message,
        },
    };

    #[tokio::test]
    async fn harness_runs_the_persisted_conversation_runtime_path() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("fixture accepts");
            let mut socket =
                accept_hdr_async(stream, |request: &Request, mut response: Response| {
                    assert_eq!(request.uri().path(), "/v1/llm/stream");
                    response.headers_mut().insert(
                        header::SEC_WEBSOCKET_PROTOCOL,
                        HeaderValue::from_static("saaa.llm-stream.v1"),
                    );
                    Ok(response)
                })
                .await
                .expect("WebSocket accepts");
            socket.send(Message::Text(serde_json::json!({
                "type":"connection.ready", "protocol":"saaa.llm-stream.v1",
                "connectionId":"quality_eval_connection", "upstreamTransport":"native",
                "limits":{"maxConcurrentRuns":1,"maxConnections":1,"maxActiveRunsPerConnection":1,
                    "maxUnackedEvents":64,"maxUnackedBytes":524288,"resumeWindowMs":120000,"heartbeatIntervalMs":15000}
            }).to_string().into())).await.expect("ready sends");
            let Message::Text(start) = socket
                .next()
                .await
                .expect("run start")
                .expect("valid run start")
            else {
                panic!("expected run.start")
            };
            let start: serde_json::Value =
                serde_json::from_str(start.as_str()).expect("run start JSON");
            assert_eq!(start["type"], "run.start");
            assert!(start["tools"].is_array());
            assert!(start["messages"].to_string().contains("SAAA Eval Agent"));
            let run_id = start["runId"].as_str().expect("run id");
            socket.send(Message::Text(serde_json::json!({"type":"run.accepted","runId":run_id,"seq":1,"providerRunId":"quality_eval_provider","model":"fixture-model"}).to_string().into())).await.expect("accepted sends");
            let content = "runtime answer";
            let mut delta = Vec::with_capacity(16 + content.len());
            delta.extend_from_slice(b"SAD1");
            delta.extend_from_slice(&[1, 0]);
            delta.extend_from_slice(&16_u16.to_be_bytes());
            delta.extend_from_slice(&2_u64.to_be_bytes());
            delta.extend_from_slice(content.as_bytes());
            socket
                .send(Message::Binary(delta.into()))
                .await
                .expect("delta sends");
            socket.send(Message::Text(serde_json::json!({"type":"response.completed","runId":run_id,"seq":3,
                "contentBytes":content.len(),"contentSha256":format!("{:x}", Sha256::digest(content.as_bytes())),
                "finishReason":"stop","usage":null}).to_string().into())).await.expect("completed sends");
            let Message::Text(ack) = socket
                .next()
                .await
                .expect("terminal ack")
                .expect("valid terminal ack")
            else {
                panic!("expected run.ack")
            };
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(ack.as_str()).expect("ack JSON")
                    ["ackSeq"],
                3
            );
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
}
