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
    let content = state
        .connection
        .lock()
        .map_err(|_| "Quality runtime database lock unavailable".to_string())?
        .query_row(
            "SELECT content FROM conversation_messages
             WHERE conversation_id=?1 AND role='assistant'
             ORDER BY rowid DESC LIMIT 1",
            [PRIMARY_CONVERSATION_ID],
            |row| row.get::<_, String>(0),
        )
        .map_err(crate::database_error)?;
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
                    "providers": [{
                        "kind": "openai-compatible",
                        "id": "quality-eval",
                        "enabled": true,
                        "label": "Quality Eval",
                        "location": location,
                        "endpoint": request.base_url,
                        "model": request.model,
                        "credentialStatus": "configured"
                    }],
                    "reasoningEffort": providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
                    "maxOutputTokens": providers::completion::DEFAULT_MAX_OUTPUT_TOKENS
                });
            }
            ("providers.agent", "codex-sdk") => {
                document.value_json["agentName"] = json!("SAAA Eval Agent");
                document.value_json["userName"] = json!("");
            }
            ("routing.tasks", "default") => {
                document.value_json["conversationRespond"] = json!({
                    "primaryProviderId": "quality-eval",
                    "fallbackProviderIds": [],
                    "timeoutMs": request.timeout_ms
                });
            }
            _ => {}
        }
    }
    persistence::save_settings_documents_to_connection(&mut connection, &documents)?;
    let situation_settings = situation::repository::load_settings(&connection)?;
    Ok(AppState {
        connection: Arc::new(Mutex::new(connection)),
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
        situation: Arc::new(situation::SituationRuntime::new(situation_settings, None)?),
        meeting: Arc::new(meeting::MeetingRuntime::new()),
        voice_profile: Arc::new(voice::profile::VoiceProfileRuntime::unavailable_for_tests(
            PathBuf::new(),
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[tokio::test]
    async fn harness_runs_the_persisted_conversation_runtime_path() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
        let address = listener.local_addr().expect("fixture address");
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("fixture accepts");
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4_096];
            let mut expected = None;
            loop {
                let count = socket.read(&mut chunk).expect("fixture reads");
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                if expected.is_none() {
                    if let Some(header_end) =
                        request.windows(4).position(|part| part == b"\r\n\r\n")
                    {
                        let headers = String::from_utf8_lossy(&request[..header_end]);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .map(str::to_string)
                            })
                            .and_then(|value| value.parse::<usize>().ok())
                            .expect("content length");
                        expected = Some(header_end + 4 + content_length);
                    }
                }
                if expected.is_some_and(|length| request.len() >= length) {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.contains("\"stream\":true"));
            assert!(request.contains("\"tools\":"));
            assert!(request.contains("SAAA Eval Agent"));
            let body = r#"{"choices":[{"index":0,"message":{"role":"assistant","content":"runtime answer"},"finish_reason":"stop"}]}"#;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("fixture responds");
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
        server.join().expect("fixture joins");
        let response: serde_json::Value = serde_json::from_str(&response).expect("response JSON");
        assert_eq!(response["content"], "runtime answer");
        assert_eq!(response["runtimePath"], "execute_turn/conversation.respond");
    }
}
