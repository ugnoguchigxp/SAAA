#![cfg(test)]

use super::*;
use crate::persistence::save_settings_documents_to_connection;
use crate::test_support::*;
use rusqlite::params;
use serde_json::{json, Value};
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
};
enum LlmHttpStep {
    Delta(&'static str),
    ToolCall {
        call_id: &'static str,
        name: &'static str,
        arguments: Value,
    },
    ExpectToolResult,
    Complete,
    Disconnect,
    Fail,
    Delay(u64),
}

async fn spawn_llm_http_fixture(
    steps: Vec<LlmHttpStep>,
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn accept(
        listener: &tokio::net::TcpListener,
        captures: &Arc<Mutex<Vec<String>>>,
    ) -> tokio::net::TcpStream {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 8192];
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
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
                    captures.lock().unwrap().push(
                        String::from_utf8(bytes[end + 4..end + 4 + length].to_vec()).unwrap(),
                    );
                    break;
                }
            }
        }
        socket
    }
    async fn start(socket: &mut tokio::net::TcpStream) {
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
    }
    async fn event(socket: &mut tokio::net::TcpStream, delta: Value, finish: Value) -> bool {
        let text = format!(
            "data: {}\n\n",
            json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
        );
        socket.write_all(text.as_bytes()).await.is_ok()
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let captures = Arc::new(Mutex::new(Vec::new()));
    let captured = captures.clone();
    let server = tokio::spawn(async move {
        let mut socket = accept(&listener, &captured).await;
        if let Some(LlmHttpStep::Fail) = steps.first() {
            socket.write_all(b"HTTP/1.1 503 Unavailable\r\nRetry-After: 60\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            return;
        }
        start(&mut socket).await;
        for step in steps {
            match step {
                    LlmHttpStep::Delta(text) => { if !event(&mut socket,json!({"content":text}),Value::Null).await {return;} }
                    LlmHttpStep::ToolCall {call_id,name,arguments} => {
                        if !event(&mut socket,json!({"tool_calls":[{"index":0,"id":call_id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}),Value::Null).await {return;}
                    }
                    LlmHttpStep::ExpectToolResult => {
                        event(&mut socket,json!({}),json!("tool_calls")).await;
                        socket.write_all(b"data: [DONE]\n\n").await.unwrap();
                        drop(socket);
                        socket=accept(&listener,&captured).await;
                        let body: Value=serde_json::from_str(captured.lock().unwrap().last().unwrap()).unwrap();
                        assert_eq!(body["messages"].as_array().unwrap().last().unwrap()["role"],"tool");
                        start(&mut socket).await;
                    }
                    LlmHttpStep::Complete => {
                        event(&mut socket,json!({}),json!("stop")).await;
                        let _=socket.write_all(b"data: [DONE]\n\n").await;
                    }
                    LlmHttpStep::Disconnect => return,
                    LlmHttpStep::Fail => panic!("failure must precede output"),
                    LlmHttpStep::Delay(ms) => tokio::time::sleep(Duration::from_millis(ms)).await,
                }
        }
    });
    (format!("http://{address}/v1"), captures, server)
}

fn begin_test_provider_session(
    state: &AppState,
    runtime_run_id: &str,
    provider_id: &str,
    provider_kind: &str,
) -> Result<String, String> {
    let fingerprint = {
        let connection = state
            .sqlite_writer
            .lock()
            .map_err(|_| "Database lock unavailable".to_string())?;
        crate::persistence::effective_route::load_conversation_configuration_fingerprint(
            &connection,
        )?
    };
    begin_provider_session(
        state,
        runtime_run_id,
        provider_id,
        provider_kind,
        &fingerprint,
    )
}

#[test]
fn duplicate_run_registration_preserves_the_original_cancellation_handle() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let original = Arc::new(RunCancellation::default());
    register_active_run(&state, "run_duplicate", original.clone()).expect("first run registers");
    let replacement = Arc::new(RunCancellation::default());
    assert!(register_active_run(&state, "run_duplicate", replacement).is_err());
    let active = state.active_runs.lock().expect("active run lock");
    assert!(Arc::ptr_eq(
        active.get("run_duplicate").expect("original run remains"),
        &original
    ));
}

#[test]
fn app_shutdown_cancels_every_active_run_and_resets_situation_to_safe_state() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let first = Arc::new(RunCancellation::default());
    let second = Arc::new(RunCancellation::default());
    register_active_run(&state, "run-close-first", first.clone()).expect("first run registers");
    register_active_run(&state, "run-close-second", second.clone()).expect("second run registers");

    state
        .situation
        .set_microphone_state(situation::contracts::MicrophoneState::SaaaCapturing);
    state
        .situation
        .set_audio_state(situation::contracts::AudioState::SaaaSpeaking);
    shutdown_app_state(&state);

    assert!(first.is_cancelled());
    assert!(second.is_cancelled());
    let snapshot = state
        .situation
        .snapshot_locked(&state.sqlite_readers)
        .expect("situation snapshot");
    assert_eq!(
        snapshot.signals.microphone.state,
        situation::contracts::MicrophoneState::Inactive
    );
    assert_eq!(
        snapshot.signals.audio.state,
        situation::contracts::AudioState::Silent
    );
}

#[test]
fn runtime_and_provider_session_finalization_is_one_shot() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
        .execute(
            "INSERT INTO conversations(id, task_mode, created_at, updated_at)
                 VALUES('conversation-finalize', 'conversation', '1', '1')",
            [],
        )
        .expect("conversation inserts");
    let state = app_state(connection);

    begin_simple_runtime_run(
        &state,
        "run-finalize",
        "conversation-finalize",
        "voice.speak",
        "provider-initial",
    )
    .expect("runtime starts");
    update_runtime_provider(&state, "run-finalize", "provider-selected")
        .expect("active runtime provider updates");
    finish_runtime_run(&state, "run-finalize", "completed", None).expect("runtime finalizes");
    assert!(finish_runtime_run(&state, "run-finalize", "failed", Some("late failure")).is_err());
    assert!(update_runtime_provider(&state, "run-finalize", "provider-late").is_err());

    let session_id = begin_test_provider_session(
        &state,
        "run-finalize",
        "provider-selected",
        "openai-compatible",
    )
    .expect("provider session starts");
    finish_provider_session(&state, &session_id, "completed", None)
        .expect("provider session finalizes");
    assert!(finish_provider_session(
        &state,
        &session_id,
        "failed",
        Some(ProviderFailureKind::Internal),
    )
    .is_err());

    let connection = state.sqlite_writer.lock().expect("database lock");
    let (status, provider_id): (String, String) = connection
        .query_row(
            "SELECT status, provider_id FROM runtime_runs WHERE id='run-finalize'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("runtime reads");
    assert_eq!(status, "completed");
    assert_eq!(provider_id, "provider-selected");
    let session_status: String = connection
        .query_row(
            "SELECT status FROM provider_sessions WHERE id=?1",
            [&session_id],
            |row| row.get(0),
        )
        .expect("provider session reads");
    assert_eq!(session_status, "completed");
}

#[test]
fn dynamic_lan_session_persists_release_success_and_deferred_cleanup() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
        .execute(
            "INSERT INTO conversations(id, task_mode, created_at, updated_at)
                 VALUES('conversation-dynamic_lan-cleanup', 'conversation', '1', '1')",
            [],
        )
        .expect("conversation inserts");
    let state = app_state(connection);

    for (run_id, cleanup, expected_status, expected_kind) in [
        (
            "run-dynamic_lan-released",
            CleanupOutcome::Released,
            "released",
            None,
        ),
        (
            "run-dynamic_lan-deferred",
            CleanupOutcome::DynamicLanDeferredToTtl { kind: "network" },
            "deferred-to-ttl",
            Some("network"),
        ),
    ] {
        begin_simple_runtime_run(
            &state,
            run_id,
            "conversation-dynamic_lan-cleanup",
            "conversation.respond",
            "lan-llm-dynamic",
        )
        .expect("runtime starts");
        let session_id =
            begin_test_provider_session(&state, run_id, "lan-llm-dynamic", "openai-compatible")
                .expect("provider session starts");
        finish_dynamic_lan_provider_session(&state, &session_id, "completed", None, cleanup)
            .expect("dynamic_lan session finalizes");
        let connection = state.sqlite_writer.lock().expect("database lock");
        let (release_status, release_kind): (String, Option<String>) = connection
            .query_row(
                "SELECT release_status, release_failure_kind FROM provider_sessions WHERE id=?1",
                [&session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("cleanup status reads");
        assert_eq!(release_status, expected_status);
        assert_eq!(release_kind.as_deref(), expected_kind);
    }
}

#[test]
fn dynamic_lan_cleanup_keeps_release_debt_across_connection_replacement() {
    let deferred = CleanupOutcome::DynamicLanDeferredToTtl { kind: "network" };
    assert_eq!(
        merge_dynamic_lan_cleanup(deferred, CleanupOutcome::Released),
        deferred
    );
    assert_eq!(
        merge_dynamic_lan_cleanup(CleanupOutcome::Released, CleanupOutcome::Released),
        CleanupOutcome::Released
    );
    assert_eq!(
        dynamic_lan_cleanup_from_release_failure(Some(providers::dynamic_lan::ErrorKind::Timeout)),
        CleanupOutcome::DynamicLanDeferredToTtl { kind: "timeout" }
    );
}

#[tokio::test]
async fn cancellation_notification_remains_observable_for_late_waiters() {
    let cancellation = RunCancellation::default();
    cancellation.cancel();

    tokio::time::timeout(Duration::from_millis(50), cancellation.cancelled())
        .await
        .expect("a cancellation sent before waiting remains observable");
}

#[test]
fn runtime_and_voice_events_serialize_camel_case_fields() {
    let runtime = serde_json::to_value(RuntimeEvent::Started {
        run_id: "run_contract".to_string(),
        route: "coding.assist".to_string(),
        provider_id: "codex-sdk".to_string(),
    })
    .expect("runtime event serializes");
    assert_eq!(runtime["type"], "started");
    assert_eq!(runtime["runId"], "run_contract");
    assert_eq!(runtime["providerId"], "codex-sdk");
    assert!(runtime.get("run_id").is_none());
    assert!(runtime.get("provider_id").is_none());
}

#[test]
fn foreign_message_flood_cannot_starve_a_request_deadline() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let producer = thread::spawn(move || {
        while sender
            .send(CodexReaderMessage::Message(json!({
                "id": 999,
                "result": {}
            })))
            .is_ok()
        {}
    });
    let origin = std::time::Instant::now();
    let mut supervisor = runtime::supervisor::RunSupervisor::new(
        runtime::contracts::RunSupervisionPolicy {
            request_timeout_ms: 20,
            progress_idle_timeout_ms: 60,
            terminal_gap_timeout_ms: 10,
            interrupt_grace_ms: 3,
            hard_timeout_ms: 300,
        },
        0,
    );
    let failure = receive_supervised_codex_result(
        &receiver,
        2,
        &mut supervisor,
        origin,
        &RunCancellation::default(),
    )
    .expect_err("foreign responses must not keep the request alive");
    drop(receiver);
    producer.join().expect("foreign response producer joins");
    assert_eq!(failure, runtime::contracts::RunFailureCode::RequestTimeout);
    assert!(origin.elapsed() < Duration::from_millis(250));
}

#[test]
fn normal_turns_reject_legacy_conversation_ids_without_writing_partial_state() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
        .execute(
            "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('legacy-conversation', 'Legacy', 'conversation', '0', '0')",
            [],
        )
        .expect("legacy conversation inserts");
    let state = app_state(connection);
    let input = StartTurnInput {
        run_id: "run-legacy-conversation".to_string(),
        conversation_id: "legacy-conversation".to_string(),
        content: "must not persist".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };

    let error = prepare_runtime_run(&state, &input).expect_err("legacy turn is rejected");
    let connection = state.sqlite_writer.lock().expect("database lock");
    let message_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_messages WHERE content = 'must not persist'",
            [],
            |row| row.get(0),
        )
        .expect("message count loads");
    let run_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runtime_runs WHERE id = 'run-legacy-conversation'",
            [],
            |row| row.get(0),
        )
        .expect("run count loads");

    assert!(error.contains("primary conversation"));
    assert_eq!(message_count, 0);
    assert_eq!(run_count, 0);
}

#[test]
fn task_specific_workspace_validation_precedes_runtime_writes() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
        .execute(
            "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('workspace-required', 'Coding', 'coding', '0', '0')",
            [],
        )
        .expect("coding conversation inserts");
    let state = app_state(connection);
    let workspace = tempfile::tempdir().expect("workspace creates");
    let normal = StartTurnInput {
        run_id: "run-normal-workspace".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "normal request".to_string(),
        workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let coding = StartTurnInput {
        run_id: "run-coding-no-workspace".to_string(),
        conversation_id: "workspace-required".to_string(),
        content: "coding request".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };

    assert!(prepare_runtime_run(&state, &normal)
        .expect_err("normal workspace is rejected")
        .contains("cannot include a workspace"));
    assert!(prepare_runtime_run(&state, &coding)
        .expect_err("coding workspace is required")
        .contains("Select a workspace"));
    let connection = state.sqlite_writer.lock().expect("database lock");
    let message_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_messages
                 WHERE content IN ('normal request', 'coding request')",
            [],
            |row| row.get(0),
        )
        .expect("message count loads");
    let run_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runtime_runs
                 WHERE id IN ('run-normal-workspace', 'run-coding-no-workspace')",
            [],
            |row| row.get(0),
        )
        .expect("run count loads");
    assert_eq!(message_count, 0);
    assert_eq!(run_count, 0);
}

#[test]
fn normal_turns_reject_byte_oversized_context_before_writing_partial_state() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let content = "😀".repeat(16_000);
    let input = StartTurnInput {
        run_id: "run-byte-oversized-context".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: content.clone(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };

    let error = prepare_runtime_run(&state, &input).expect_err("oversized turn is rejected");
    let connection = state.sqlite_writer.lock().expect("database lock");
    let message_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_messages WHERE content = ?1",
            params![content],
            |row| row.get(0),
        )
        .expect("message count loads");
    let run_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM runtime_runs WHERE id = 'run-byte-oversized-context'",
            [],
            |row| row.get(0),
        )
        .expect("run count loads");

    assert!(error.contains("too large"));
    assert_eq!(message_count, 0);
    assert_eq!(run_count, 0);
}

#[test]
fn readiness_data_directory_is_isolated_and_permission_bounded() {
    let directory = tempfile::tempdir().expect("temporary directory creates");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("temporary directory permissions are isolated");
    }
    let normal = directory.path().join("normal-app-data");
    let resolved = app_paths::validate_readiness_data_directory(directory.path(), &normal)
        .expect("isolated directory is accepted");
    assert_eq!(
        resolved,
        directory.path().canonicalize().expect("path resolves")
    );
    assert!(
        app_paths::validate_readiness_data_directory(directory.path(), directory.path())
            .expect_err("normal app data is rejected")
            .contains("normal application data")
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755))
            .expect("permissions change");
        assert!(
            app_paths::validate_readiness_data_directory(directory.path(), &normal)
                .expect_err("broad permissions are rejected")
                .contains("mode 0700")
        );
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("permissions restore");
    }
}

#[tokio::test]
async fn openai_compatible_stream_fixture_projects_deltas() {
    let (endpoint, request_body, server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta("hello "),
        LlmHttpStep::Delta("world"),
        LlmHttpStep::Complete,
    ])
    .await;
    let provider = OpenAiCompatibleProviderSettings {
        endpoint,
        ..direct_provider("stream-fixture", "local")
    };
    let input = StartTurnInput {
        run_id: "run-stream-fixture".to_string(),
        conversation_id: "conversation-fixture".to_string(),
        content: "hello".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let history = vec![ConversationMessage {
        parts: None,
        id: "message-fixture".to_string(),
        conversation_id: input.conversation_id.clone(),
        role: "user".to_string(),
        content: input.content.clone(),
        created_at: "now".to_string(),
    }];
    let projected = Arc::new(Mutex::new(Vec::<String>::new()));
    let projected_for_channel = projected.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            projected_for_channel
                .lock()
                .expect("projection lock")
                .push(value);
        }
        Ok(())
    });
    let content = stream_model_provider_with_api_key(
        &provider,
        &history,
        5_000,
        Some("ephemeral-connection-token"),
        None, // Direct SSE fixture; allocation-backed connections use JSON completions.
        ModelStreamContext {
            reasoning_effort: "low",
            max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &channel,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
    )
    .await;
    let ProviderAttemptOutcome::Completed { content, .. } = content else {
        panic!("provider stream should complete: {content:?}");
    };
    server.await.expect("fixture server joins");
    assert_eq!(content, "hello world");
    let request = request_body.lock().expect("request lock")[0].clone();
    assert!(request.contains("\"stream\":true"));
    assert!(!request.contains("allocationId"));
    assert!(!request.contains("reasoning_effort"));
    assert!(request.contains("\"max_tokens\":2048"));
    assert_eq!(
        projected
            .lock()
            .expect("projection lock")
            .iter()
            .filter(|event| event.contains("\"type\":\"delta\""))
            .count(),
        2
    );
}

#[tokio::test]
async fn http_disconnect_preserves_partial_output_without_regeneration() {
    let (endpoint, captures, server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta("before "),
        LlmHttpStep::Disconnect,
        LlmHttpStep::Delta("after"),
        LlmHttpStep::Complete,
    ])
    .await;
    let provider = OpenAiCompatibleProviderSettings {
        endpoint,
        ..direct_provider("resume-fixture", "local")
    };
    let input = StartTurnInput {
        run_id: "run-resume-fixture".to_string(),
        conversation_id: "conversation-resume-fixture".to_string(),
        content: "resume".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let deltas = Arc::new(Mutex::new(Vec::<String>::new()));
    let projected = deltas.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            if value.contains("\"type\":\"delta\"") {
                projected.lock().expect("delta lock").push(value);
            }
        }
        Ok(())
    });
    let outcome = stream_model_provider(
        &provider,
        &[],
        5_000,
        ModelStreamContext {
            reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &channel,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
    )
    .await;
    server.await.expect("resume fixture joins");
    assert!(matches!(
        outcome,
        ProviderAttemptOutcome::Failed {
            output_started: true,
            ..
        }
    ));
    let projected = deltas.lock().expect("delta lock").join("\n");
    assert_eq!(projected.matches("before ").count(), 1);
    assert_eq!(projected.matches("after").count(), 0);
    assert_eq!(captures.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn dynamic_lan_stream_policy_requires_sse_for_stream_requests() {
    use std::io::{Read, Write as _};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("fixture accepts request");
        let mut request = [0_u8; 8_192];
        let _ = socket.read(&mut request).expect("fixture reads request");
        let body = r#"{"choices":[{"message":{"content":"not streamed"}}]}"#;
        write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("fixture writes response");
    });
    let provider = OpenAiCompatibleProviderSettings {
        request_options: None,
        endpoint: format!("http://{address}/v1"),
        ..direct_provider("dynamic_lan-sse-policy", "local")
    };
    let input = StartTurnInput {
        run_id: "run-dynamic_lan-sse-policy".to_string(),
        conversation_id: "conversation-dynamic_lan-sse-policy".to_string(),
        content: "test".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let history = vec![ConversationMessage {
        parts: None,
        id: "message-dynamic_lan-sse-policy".to_string(),
        conversation_id: input.conversation_id.clone(),
        role: "user".to_string(),
        content: input.content.clone(),
        created_at: "now".to_string(),
    }];
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let outcome = stream_model_provider_with_api_key(
        &provider,
        &history,
        5_000,
        Some("ephemeral-connection-token"),
        None,
        ModelStreamContext {
            reasoning_effort: "low",
            max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &channel,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
    )
    .await;
    server.join().expect("fixture server joins");
    assert!(matches!(
        outcome,
        ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::Protocol | ProviderFailureKind::Network,
            output_started: false,
            ..
        }
    ));
}

#[tokio::test]
async fn openai_provider_executes_the_single_recall_tool_before_final_output() {
    let (endpoint, captures, server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::ToolCall {
            call_id: "call_recall_1",
            name: "recall_conversation",
            arguments: json!({ "query": "SQLite" }),
        },
        LlmHttpStep::ExpectToolResult,
        LlmHttpStep::Delta("履歴を確認しました"),
        LlmHttpStep::Complete,
    ])
    .await;

    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
            .execute_batch(
                "INSERT INTO conversations(id,task_mode,created_at,updated_at)
                   VALUES('conversation-old','conversation','1','1');
                 INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                   VALUES('message-old-user','conversation-old','user','SQLite の検索方式を相談した','1000');
                 INSERT INTO conversation_messages(id,conversation_id,role,content,created_at)
                   VALUES('message-old-assistant','conversation-old','assistant','FTS と時間条件を組み合わせます','1001');",
            )
            .expect("history inserts");
    let state = app_state(connection);
    let input = StartTurnInput {
        run_id: "run-recall-tool".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "前の話を思い出して".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    prepare_runtime_run(&state, &input).expect("runtime prepares");
    let session_id =
        begin_test_provider_session(&state, &input.run_id, "recall-fixture", "openai-compatible")
            .expect("provider session starts");
    let history = list_messages_from_connection(
        &state.sqlite_writer.lock().expect("database lock"),
        &input.conversation_id,
    )
    .expect("history loads");
    let provider = OpenAiCompatibleProviderSettings {
        endpoint,
        ..direct_provider("recall-fixture", "local")
    };
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let outcome = stream_model_provider(
        &provider,
        &history,
        5_000,
        ModelStreamContext {
            reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &channel,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: Some(ProviderOutputPersistence {
                state: &state,
                session_id: &session_id,
                world: None,
            }),
        },
    )
    .await;
    server.await.expect("fixture server joins");

    let ProviderAttemptOutcome::Completed { content, .. } = outcome else {
        panic!("tool-assisted provider stream should complete");
    };
    assert_eq!(content, "履歴を確認しました");
    let captures = captures.lock().expect("capture lock");
    assert_eq!(captures.len(), 2);
    let first: Value = serde_json::from_str(&captures[0]).expect("run.start JSON");
    assert_eq!(first["tools"].as_array().expect("tools array").len(), 4);
    assert_eq!(
        first
            .pointer("/tools/0/function/name")
            .and_then(Value::as_str),
        Some("recall_conversation")
    );
    assert_eq!(
        first
            .pointer("/tools/3/function/name")
            .and_then(Value::as_str),
        Some("update_conversation_voice_behavior")
    );
    let continuation: Value = serde_json::from_str(&captures[1]).expect("continuation JSON");
    let tool_result = continuation["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(tool_result["role"], "tool");
    assert!(tool_result["content"]
        .as_str()
        .is_some_and(|content| content.contains("SQLite の検索方式")));
    assert!(!tool_result["content"]
        .as_str()
        .is_some_and(|content| content.contains("前の話を思い出して")));
    let connection = state.sqlite_writer.lock().expect("database lock");
    let receipts: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_recall_receipts WHERE runtime_run_id=?1",
            [&input.run_id],
            |row| row.get(0),
        )
        .expect("receipt count reads");
    assert_eq!(receipts, 1);
}

#[test]
fn voice_policy_tool_quota_is_independent_from_other_agent_tools() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let input = StartTurnInput {
        run_id: "run-voice-quota".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "quiet please".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual-and-spoken".to_string(),
    };
    let persistence = Some(ProviderOutputPersistence {
        state: &state,
        session_id: "unused-session",
        world: None,
    });

    let after_general_quota = available_agent_tools(persistence, &input, 12, 0, 0);
    assert!(tool_was_offered(
        &after_general_quota.definitions,
        voice_behavior::UPDATE_VOICE_BEHAVIOR_TOOL_NAME
    ));
    assert!(tool_was_offered(
        &after_general_quota.definitions,
        "recall_conversation"
    ));
    assert!(tool_was_offered(
        &after_general_quota.definitions,
        "web_search"
    ));

    let after_voice_quota = available_agent_tools(persistence, &input, 1, 1, 0);
    assert!(!tool_was_offered(
        &after_voice_quota.definitions,
        voice_behavior::UPDATE_VOICE_BEHAVIOR_TOOL_NAME
    ));
    assert!(tool_was_offered(
        &after_voice_quota.definitions,
        "recall_conversation"
    ));
}

#[tokio::test]
async fn typed_memory_tools_are_routed_only_from_a_valid_typed_manifest() {
    let directory = tempfile::tempdir().expect("temporary run directory creates");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("run directory permissions set");
    }
    let token_path = directory.path().join("mcp-memory-bearer.token");
    fs::write(
        &token_path,
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
    )
    .expect("test token writes");
    let manifest_path = directory.path().join("mcp-endpoint.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec(&json!({
            "server": "context-still",
            "url": "http://127.0.0.1:39173/mcp",
            "transport": "streamable-http",
            "protocolVersion": "2025-03-26",
            "auth": "bearer-token-file",
            "authTokenPath": token_path,
            "toolProfile": "typed-memory",
            "contractVersion": "memory-recall-v1",
            "startedAt": "unix-ms:1"
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");
    #[cfg(unix)]
    for path in [&token_path, &manifest_path] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("file permissions set");
    }

    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let mut state = app_state(connection);
    state.context_still_recall =
        memory::context_still_recall::ContextStillRecallClient::with_run_dir(
            directory.path().to_path_buf(),
            true,
        );
    let input = StartTurnInput {
        run_id: "run-typed-routing".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "remember a rule".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let persistence = Some(ProviderOutputPersistence {
        state: &state,
        session_id: "unused-session",
        world: None,
    });
    let names = available_agent_tools(persistence, &input, 0, 0, 0)
        .definitions
        .into_iter()
        .map(|definition| {
            definition
                .pointer("/function/name")
                .and_then(Value::as_str)
                .expect("tool name exists")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "recall_conversation",
            "recall_experience",
            "recall_rule",
            "recall_skill",
            "web_search",
            "fetch_content",
            "update_conversation_voice_behavior"
        ]
    );

    let after_typed_memory_quota = available_agent_tools(
        persistence,
        &input,
        memory::typed_recall::MAX_TYPED_RECALL_CALLS_PER_TURN,
        0,
        memory::typed_recall::MAX_TYPED_RECALL_CALLS_PER_TURN,
    );
    for name in ["recall_experience", "recall_rule", "recall_skill"] {
        assert!(!tool_was_offered(
            &after_typed_memory_quota.definitions,
            name
        ));
    }
    assert!(tool_was_offered(
        &after_typed_memory_quota.definitions,
        "recall_conversation"
    ));
    assert!(tool_was_offered(
        &after_typed_memory_quota.definitions,
        "web_search"
    ));

    let error = execute_agent_tool(
        persistence,
        &input,
        &runtime::agent_tools::AgentToolCall {
            id: "call-typed-invalid".to_string(),
            name: "recall_rule".to_string(),
            arguments: r#"{"query":"release","projectRef":"forbidden"}"#.to_string(),
        },
        Duration::from_secs(1),
        &crate::generated_capabilities::publication::GeneratedToolSnapshot::empty(),
        &crate::RunCancellation::default(),
    )
    .await;
    assert!(error.contains("invalid-memory-input"));
    assert!(!error.contains("forbidden"));
}

#[tokio::test]
async fn typed_memory_execution_cannot_exceed_the_provider_deadline() {
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture binds");
    listener.set_nonblocking(true).expect("fixture is bounded");
    let address = listener.local_addr().expect("fixture address");
    let (release_sender, release_receiver) = mpsc::channel();
    let server = thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(error) => panic!("fixture accept failed: {error}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("fixture read is bounded");
        let mut request = [0_u8; 4 * 1_024];
        let _ = socket.read(&mut request);
        let _ = release_receiver.recv_timeout(Duration::from_secs(2));
    });

    let directory = tempfile::tempdir().expect("run directory creates");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("run directory permissions set");
    }
    let token_path = directory.path().join("mcp-memory-bearer.token");
    let manifest_path = directory.path().join("mcp-endpoint.json");
    fs::write(
        &token_path,
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n",
    )
    .expect("token writes");
    fs::write(
        &manifest_path,
        serde_json::to_vec(&json!({
            "server": "context-still",
            "url": format!("http://{address}/mcp"),
            "transport": "streamable-http",
            "protocolVersion": "2025-03-26",
            "auth": "bearer-token-file",
            "authTokenPath": token_path,
            "toolProfile": "typed-memory",
            "contractVersion": "memory-recall-v1",
            "startedAt": "unix-ms:1"
        }))
        .expect("manifest encodes"),
    )
    .expect("manifest writes");
    #[cfg(unix)]
    for path in [&token_path, &manifest_path] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .expect("fixture permissions set");
    }

    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let mut state = app_state(connection);
    state.context_still_recall =
        memory::context_still_recall::ContextStillRecallClient::with_run_dir(
            directory.path().to_path_buf(),
            true,
        );
    let input = StartTurnInput {
        run_id: "run-typed-memory-timeout".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "remember".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let persistence = Some(ProviderOutputPersistence {
        state: &state,
        session_id: "session-typed-memory-timeout",
        world: None,
    });
    let call = runtime::agent_tools::AgentToolCall {
        id: "call-typed-timeout".to_string(),
        name: "recall_rule".to_string(),
        arguments: r#"{"query":"release"}"#.to_string(),
    };
    let generated = crate::generated_capabilities::publication::GeneratedToolSnapshot::empty();
    let cancellation = crate::RunCancellation::default();
    let execution = execute_agent_tool(
        persistence,
        &input,
        &call,
        Duration::from_millis(20),
        &generated,
        &cancellation,
    );
    let error = tokio::time::timeout(Duration::from_millis(250), execution)
        .await
        .expect("typed recall respects the provider deadline");
    assert!(error.contains("typed-memory-unavailable"));

    release_sender.send(()).expect("fixture releases");
    server.join().expect("fixture joins");
}

#[test]
fn malformed_recall_calls_consume_the_persistent_turn_limit() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let input = StartTurnInput {
        run_id: "run-malformed-recall".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "remember".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    prepare_runtime_run(&state, &input).expect("runtime prepares");
    let persistence = Some(ProviderOutputPersistence {
        state: &state,
        session_id: "unused-session",
        world: None,
    });
    for index in 0..3 {
        let content = execute_recall_tool(
            persistence,
            &input,
            &runtime::agent_tools::AgentToolCall {
                id: format!("call_malformed_{index}"),
                name: "recall_conversation".to_string(),
                arguments: "{".to_string(),
            },
        );
        assert!(content.contains("invalid-input"));
    }
    let limited = execute_recall_tool(
        persistence,
        &input,
        &runtime::agent_tools::AgentToolCall {
            id: "call_malformed_4".to_string(),
            name: "recall_conversation".to_string(),
            arguments: "{".to_string(),
        },
    );
    assert!(limited.contains("call-limit-exceeded"));
    let attempts: i64 = state
        .sqlite_writer
        .lock()
        .expect("database lock")
        .query_row(
            "SELECT COUNT(*) FROM conversation_recall_attempts
                 WHERE runtime_run_id=?1",
            [&input.run_id],
            |row| row.get(0),
        )
        .expect("attempt count reads");
    assert_eq!(attempts, 3);
}

#[tokio::test]
async fn recall_tool_rounds_share_one_provider_timeout_budget() {
    let (endpoint, _, server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delay(350),
        LlmHttpStep::ToolCall {
            call_id: "call_timeout",
            name: "recall_conversation",
            arguments: json!({"query": "missing"}),
        },
        LlmHttpStep::ExpectToolResult,
        LlmHttpStep::Delay(350),
        LlmHttpStep::Delta("too late"),
        LlmHttpStep::Complete,
    ])
    .await;

    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let input = StartTurnInput {
        run_id: "run-recall-timeout".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "remember".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    prepare_runtime_run(&state, &input).expect("runtime prepares");
    let session_id = begin_test_provider_session(
        &state,
        &input.run_id,
        "timeout-fixture",
        "openai-compatible",
    )
    .expect("provider session starts");
    let history = list_messages_from_connection(
        &state.sqlite_writer.lock().expect("database lock"),
        &input.conversation_id,
    )
    .expect("history loads");
    let provider = OpenAiCompatibleProviderSettings {
        endpoint,
        ..direct_provider("timeout-fixture", "local")
    };
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let outcome = stream_model_provider(
        &provider,
        &history,
        600,
        ModelStreamContext {
            reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &channel,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: Some(ProviderOutputPersistence {
                state: &state,
                session_id: &session_id,
                world: None,
            }),
        },
    )
    .await;
    server.await.expect("fixture server joins");
    assert!(matches!(
        outcome,
        ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::Timeout,
            output_started: true,
            ..
        }
    ));
}

#[tokio::test]
async fn provider_stream_stops_when_the_tauri_consumer_disconnects() {
    let (endpoint, _, server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta("visible"),
        LlmHttpStep::Delta("ignored"),
        LlmHttpStep::Complete,
    ])
    .await;
    let provider = OpenAiCompatibleProviderSettings {
        endpoint,
        ..direct_provider("consumer-disconnect", "local")
    };
    let input = StartTurnInput {
        run_id: "run-consumer-disconnect".to_string(),
        conversation_id: "conversation-consumer-disconnect".to_string(),
        content: "hello".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let channel: tauri::ipc::Channel<RuntimeEvent> =
        tauri::ipc::Channel::new(|_| Err(tauri::Error::Io(std::io::Error::other("closed"))));
    let outcome = stream_model_provider(
        &provider,
        &[],
        2_000,
        ModelStreamContext {
            reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &channel,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
    )
    .await;
    server.await.expect("fixture server joins");
    assert!(matches!(
        outcome,
        ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::ClientDisconnected,
            output_started: true,
            ..
        }
    ));
}

#[tokio::test]
async fn model_provider_redirects_are_not_followed() {
    use std::io::{ErrorKind, Read, Write as _};
    use std::net::TcpListener;

    let redirect_target = TcpListener::bind("127.0.0.1:0").expect("target binds");
    let target_address = redirect_target.local_addr().expect("target address");
    redirect_target
        .set_nonblocking(true)
        .expect("target becomes nonblocking");
    let target_hit = Arc::new(AtomicBool::new(false));
    let target_hit_for_server = target_hit.clone();
    let target_server = thread::spawn(move || {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            match redirect_target.accept() {
                Ok(_) => {
                    target_hit_for_server.store(true, Ordering::SeqCst);
                    return;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    let source = TcpListener::bind("127.0.0.1:0").expect("source binds");
    let source_address = source.local_addr().expect("source address");
    let source_server = thread::spawn(move || {
        let (mut socket, _) = source.accept().expect("source accepts request");
        let mut request = [0; 4_096];
        let _ = socket.read(&mut request).expect("source reads request");
        let response = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target_address}/v1/chat/completions\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
        socket
            .write_all(response.as_bytes())
            .expect("source writes redirect");
    });
    let provider = OpenAiCompatibleProviderSettings {
        request_options: None,
        endpoint: format!("http://{source_address}/v1"),
        ..direct_provider("redirect-fixture", "local")
    };
    let input = StartTurnInput {
        run_id: "run-redirect-fixture".to_string(),
        conversation_id: "conversation-fixture".to_string(),
        content: "hello".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let outcome = stream_model_provider(
        &provider,
        &[],
        2_000,
        ModelStreamContext {
            reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &channel,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
    )
    .await;
    source_server.join().expect("source server joins");
    target_server.join().expect("target server joins");
    assert!(matches!(
        outcome,
        ProviderAttemptOutcome::Failed {
            kind: ProviderFailureKind::Contract | ProviderFailureKind::Network,
            output_started: false,
            ..
        }
    ));
    assert!(!target_hit.load(Ordering::SeqCst));
}

#[tokio::test]
async fn conversation_route_falls_back_and_persists_completed_message() {
    let (primary_endpoint, _, primary_server) =
        spawn_llm_http_fixture(vec![LlmHttpStep::Fail]).await;
    let (fallback_endpoint, _, fallback_server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta("fallback ok"),
        LlmHttpStep::Complete,
    ])
    .await;
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let conversation = Conversation {
        id: PRIMARY_CONVERSATION_ID.to_string(),
        title: Some(PRIMARY_CONVERSATION_TITLE.to_string()),
        task_mode: "conversation".to_string(),
        created_at: "now".to_string(),
        updated_at: "now".to_string(),
    };
    let mut documents = default_settings_input();
    documents
        .iter_mut()
        .find(|document| document.namespace == "providers.model")
        .expect("provider settings")
        .value_json = json!({
            "harness": { "address": "http://localhost:9810" }, "providers": [{
            "kind": "openai-compatible", "id": "primary", "enabled": true, "label": "Primary", "location": "local",
            "endpoint": primary_endpoint, "model": "primary-model", "authentication": "none"
        }, {
            "kind": "openai-compatible", "id": "fallback", "enabled": true, "label": "Fallback", "location": "local",
            "endpoint": fallback_endpoint, "model": "fallback-model", "authentication": "none"
        }], "reasoningEffort": "medium"});
    let route = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.tasks")
        .expect("route settings");
    route.value_json["conversationRespond"]["source"] = json!("provider");
    route.value_json["conversationRespond"]["primaryProviderId"] = json!("primary");
    route.value_json["conversationRespond"]["fallbackProviderIds"] = json!(["fallback"]);
    route.value_json["voiceSpeak"]["source"] = json!("harness");
    route.value_json["voiceSpeak"]["providerId"] = Value::Null;
    save_settings_documents_to_connection(
        &mut state.sqlite_writer.lock().expect("database lock"),
        &documents,
    )
    .expect("settings save");
    let input = StartTurnInput {
        run_id: "run-fallback".to_string(),
        conversation_id: conversation.id,
        content: "test fallback".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let events_for_channel = events.clone();
    let completed_states = Arc::new(Mutex::new(Vec::<String>::new()));
    let completed_states_for_channel = completed_states.clone();
    let delta_states = Arc::new(Mutex::new(Vec::<i64>::new()));
    let delta_states_for_channel = delta_states.clone();
    let database_for_channel = state.sqlite_writer.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            if value.contains("\"type\":\"messageCompleted\"") {
                let status = database_for_channel
                    .lock()
                    .expect("database lock")
                    .query_row(
                        "SELECT status FROM runtime_runs WHERE id='run-fallback'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("committed run is readable from callback");
                completed_states_for_channel
                    .lock()
                    .expect("completed state lock")
                    .push(status);
            } else if value.contains("\"type\":\"delta\"") {
                let output_started = database_for_channel
                    .lock()
                    .expect("database lock")
                    .query_row(
                        "SELECT output_started FROM provider_sessions
                             WHERE runtime_run_id='run-fallback' AND provider_id='fallback'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("committed output state is readable from callback");
                delta_states_for_channel
                    .lock()
                    .expect("delta state lock")
                    .push(output_started);
            }
            events_for_channel.lock().expect("event lock").push(value);
        }
        Ok(())
    });
    execute_turn(
        &state,
        &input,
        &channel,
        Arc::new(RunCancellation::default()),
        None,
    )
    .await
    .expect("fallback completes");
    primary_server.await.expect("primary server joins");
    fallback_server.await.expect("fallback server joins");
    let messages = list_messages_from_connection(
        &state.sqlite_writer.lock().expect("database lock"),
        &input.conversation_id,
    )
    .expect("messages load");
    assert_eq!(
        messages.last().expect("assistant message").content,
        "fallback ok"
    );
    let projected = events.lock().expect("event lock").join("\n");
    assert!(projected.contains("providerFailed"));
    assert!(projected.contains("messageCompleted"));
    assert_eq!(projected.matches("\"kind\":\"context-window\"").count(), 1);
    assert!(projected.contains("Context green:"));
    assert_eq!(
        *completed_states.lock().expect("completed state lock"),
        vec!["completed"]
    );
    assert_eq!(*delta_states.lock().expect("delta state lock"), vec![1]);
}

#[tokio::test]
async fn partial_provider_stream_never_reaches_the_fallback_provider() {
    use std::io::ErrorKind;
    use std::net::TcpListener;

    let (primary_endpoint, _, primary_server) =
        spawn_llm_http_fixture(vec![LlmHttpStep::Delta("partial"), LlmHttpStep::Disconnect]).await;

    let fallback = TcpListener::bind("127.0.0.1:0").expect("fallback binds");
    let fallback_address = fallback.local_addr().expect("fallback address");
    fallback
        .set_nonblocking(true)
        .expect("fallback becomes nonblocking");
    let fallback_hit = Arc::new(AtomicBool::new(false));
    let fallback_hit_for_server = fallback_hit.clone();
    let fallback_server = thread::spawn(move || {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            match fallback.accept() {
                Ok(_) => {
                    fallback_hit_for_server.store(true, Ordering::SeqCst);
                    return;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });

    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let mut documents = default_settings_input();
    documents
        .iter_mut()
        .find(|document| document.namespace == "providers.model")
        .expect("provider settings")
        .value_json = json!({
            "harness": { "address": "http://localhost:9810" }, "providers": [{
            "kind": "openai-compatible", "id": "partial-primary", "enabled": true, "label": "Partial primary", "location": "local",
            "endpoint": primary_endpoint, "model": "primary-model", "authentication": "none"
        }, {
            "kind": "openai-compatible", "id": "forbidden-fallback", "enabled": true, "label": "Forbidden fallback", "location": "local",
            "endpoint": format!("http://{fallback_address}/v1"), "model": "fallback-model", "authentication": "none"
        }], "reasoningEffort": "medium"});
    let route = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.tasks")
        .expect("routing settings");
    route.value_json["conversationRespond"]["source"] = json!("provider");
    route.value_json["conversationRespond"]["primaryProviderId"] = json!("partial-primary");
    route.value_json["conversationRespond"]["fallbackProviderIds"] = json!(["forbidden-fallback"]);
    route.value_json["voiceSpeak"]["source"] = json!("harness");
    route.value_json["voiceSpeak"]["providerId"] = Value::Null;
    save_settings_documents_to_connection(
        &mut state.sqlite_writer.lock().expect("database lock"),
        &documents,
    )
    .expect("settings save");
    let input = StartTurnInput {
        run_id: "run-partial".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "partial test".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    execute_turn(
        &state,
        &input,
        &channel,
        Arc::new(RunCancellation::default()),
        None,
    )
    .await
    .expect_err("partial stream fails the turn");
    primary_server.await.expect("primary server joins");
    fallback_server.join().expect("fallback server joins");

    assert!(!fallback_hit.load(Ordering::SeqCst));
    let connection = state.sqlite_writer.lock().expect("database lock");
    let assistant_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_messages
                 WHERE conversation_id=?1 AND role='assistant'",
            params![PRIMARY_CONVERSATION_ID],
            |row| row.get(0),
        )
        .expect("assistant count reads");
    let (session_count, failure_reason): (i64, String) = connection
        .query_row(
            "SELECT COUNT(*), MAX(failure_reason) FROM provider_sessions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("provider session reads");
    assert_eq!(assistant_count, 0);
    assert_eq!(session_count, 1);
    assert_eq!(failure_reason, "network");
}

#[test]
fn codex_thread_mapping_survives_database_reopen() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("threads.sqlite3");
    let connection = Connection::open(&path).expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
        .execute(
            "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('coding-conversation', NULL, 'coding', 'now', 'now')",
            [],
        )
        .expect("conversation inserts");
    let state = app_state(connection);
    persist_codex_thread(
        &state,
        "coding-conversation",
        "thread-persisted",
        "gpt-test",
        directory.path(),
    )
    .expect("thread persists");
    drop(state);

    let reopened = Connection::open(path).expect("database reopens");
    initialize_database(&reopened).expect("database reinitializes");
    let thread_id: String = reopened
        .query_row(
            "SELECT thread_id FROM codex_threads WHERE conversation_id = 'coding-conversation'",
            [],
            |row| row.get(0),
        )
        .expect("thread reloads");
    assert_eq!(thread_id, "thread-persisted");
}

#[test]
fn audio_resampling_and_cancellation_are_bounded_and_idempotent() {
    let input = (0..48_000)
        .map(|index| (index as f32 / 48_000.0) * 2.0 - 1.0)
        .collect::<Vec<_>>();
    let output = voice::network_asr::resample_pcm(&input, 48_000, 16_000);
    assert_eq!(output.len(), 16_000);
    assert!(output.iter().all(|sample| (-1.0..=1.0).contains(sample)));
    let cancellation = RunCancellation::default();
    cancellation.cancel();
    cancellation.cancel();
    assert!(cancellation.is_cancelled());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_system_tts_runtime_is_available() {
    assert!(Command::new("say")
        .args(["-v", "?"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("system say command starts")
        .success());
}

#[cfg(unix)]
#[test]
fn codex_app_server_contract_covers_start_stream_resume_and_cancel() {
    let _lock = crate::test_environment::codex_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temporary directory");
    let executable = directory.path().join("codex-fixture.py");
    let log_path = directory.path().join("requests.jsonl");
    let quoted_log_path =
        serde_json::to_string(&log_path.to_string_lossy()).expect("fixture log path encodes");
    let fixture = format!(
        r#"#!/usr/bin/env python3
import json, sys
log_path = {quoted_log_path}
scenario = "normal"
with open(log_path, "a", encoding="utf-8") as log:
    log.write(json.dumps({{"argv": sys.argv[1:]}}) + "\n")
for line in sys.stdin:
    message = json.loads(line)
    with open(log_path, "a", encoding="utf-8") as log:
        log.write(json.dumps(message) + "\n")
    request_id = message.get("id")
    method = message.get("method")
    if request_id == 1:
        print(json.dumps({{"id": 1, "result": {{}}}}), flush=True)
    elif request_id == 2:
        scenario = message.get("params", {{}}).get("model", "normal")
        if scenario != "thread-hang":
            thread_id = "x" * 161 if scenario == "invalid-thread-id" else "fixture-thread"
            print(json.dumps({{"id": 2, "result": {{"thread": {{"id": thread_id}}}}}}), flush=True)
    elif request_id == 3:
        text = message.get("params", {{}}).get("input", [{{}}])[0].get("text", "")
        if scenario != "turn-hang":
            turn_id = "x" * 161 if scenario == "invalid-turn-id" else "fixture-turn"
            print(json.dumps({{"id": 3, "result": {{"turn": {{"id": turn_id}}}}}}), flush=True)
        if scenario == "malformed":
            print("{{not-json", flush=True)
        elif scenario == "provider-error":
            print(json.dumps({{"method": "error", "params": {{"message": "SAAA_PRIVATE_PROVIDER_DETAIL"}}}}), flush=True)
        elif scenario == "terminal-failed":
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "failed", "error": {{"message": "SAAA_PRIVATE_TERMINAL_DETAIL"}}}}}}}}), flush=True)
        elif scenario == "approval":
            print(json.dumps({{"id": 99, "method": "item/requestApproval", "params": {{}}}}), flush=True)
        elif scenario in ["fileChange", "mcpToolCall", "dynamicToolCall", "webSearch"]:
            print(json.dumps({{"method": "item/started", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "forbidden_1", "type": scenario}}}}}}), flush=True)
        elif scenario == "terminal-hang":
            print(json.dumps({{"method": "item/started", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage"}}}}}}), flush=True)
            print(json.dumps({{"method": "item/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage", "text": "SAAA_TERMINAL_WAIT"}}}}}}), flush=True)
        elif scenario == "foreign":
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"threadId": "other", "turnId": "other", "delta": "foreign"}}}}), flush=True)
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"delta": "unscoped"}}}}), flush=True)
        elif scenario == "duplicate":
            started = {{"method": "item/started", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage"}}}}}}
            completed = {{"method": "item/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage", "text": "SAAA_DUPLICATE_OK"}}}}}}
            print(json.dumps(started), flush=True)
            print(json.dumps(started), flush=True)
            print(json.dumps(completed), flush=True)
            print(json.dumps(completed), flush=True)
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "completed"}}}}}}), flush=True)
        elif scenario == "response-too-large":
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "delta": "x" * 64001}}}}), flush=True)
        elif scenario == "stream-too-large":
            print(json.dumps({{"method": "unknown", "params": {{"blob": "x" * (4 * 1024 * 1024)}}}}), flush=True)
        elif scenario == "child-exit":
            sys.exit(3)
        elif scenario not in ["progress-hang", "hard-hang", "cancel-no-response"] and "CANCEL" not in text:
            reply = "SAAA_RESUMED" if method == "turn/start" and "RESUME" in text else "SAAA_OK"
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "delta": reply}}}}), flush=True)
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "completed"}}}}}}), flush=True)
    elif method == "turn/interrupt":
        if scenario != "cancel-no-response":
            print(json.dumps({{"id": request_id, "result": {{}}}}), flush=True)
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "interrupted"}}}}}}), flush=True)
"#,
    );
    fs::write(&executable, fixture).expect("fixture writes");
    let mut permissions = fs::metadata(&executable)
        .expect("fixture metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).expect("fixture becomes executable");

    let _codex_environment = crate::test_environment::EnvGuard::set("SAAA_CODEX_PATH", &executable);
    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let received_for_channel = received.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            received_for_channel.lock().expect("event lock").push(value);
        }
        Ok(())
    });
    let cancellation = RunCancellation::default();
    let first = run_codex_turn_process(
        "run-start",
        "START",
        directory.path(),
        "gpt-fixture",
        None,
        10_000,
        &channel,
        &cancellation,
    )
    .expect("start turn succeeds");
    assert_eq!(first.thread_id, "fixture-thread");
    assert_eq!(first.content, "SAAA_OK");
    let resumed = run_codex_turn_process(
        "run-resume",
        "RESUME",
        directory.path(),
        "gpt-fixture",
        Some(&first.thread_id),
        10_000,
        &channel,
        &cancellation,
    )
    .expect("resume turn succeeds");
    assert_eq!(resumed.content, "SAAA_RESUMED");
    assert_eq!(
        run_codex_turn_process(
            "run-invalid-resume",
            "RESUME",
            directory.path(),
            "gpt-fixture",
            Some(&"x".repeat(161)),
            10_000,
            &channel,
            &cancellation,
        )
        .expect_err("invalid persisted thread id fails closed")
        .code,
        runtime::contracts::RunFailureCode::ProtocolError
    );
    let cancelled = Arc::new(RunCancellation::default());
    let cancel_trigger = cancelled.clone();
    let cancel_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        cancel_trigger.cancel();
    });
    let cancellation_result = run_codex_turn_process(
        "run-cancel",
        "CANCEL",
        directory.path(),
        "gpt-fixture",
        Some(&first.thread_id),
        10_000,
        &channel,
        &cancelled,
    );
    cancel_thread.join().expect("cancel trigger joins");

    let policy = runtime::contracts::RunSupervisionPolicy {
        request_timeout_ms: 200,
        progress_idle_timeout_ms: 40,
        terminal_gap_timeout_ms: 30,
        interrupt_grace_ms: 20,
        hard_timeout_ms: 200,
    };
    let interrupt_count = || {
        fs::read_to_string(&log_path)
            .expect("fixture log loads")
            .matches("\"method\": \"turn/interrupt\"")
            .count()
    };
    let run_scenario = |scenario: &str,
                        scenario_policy: runtime::contracts::RunSupervisionPolicy,
                        cancellation: &RunCancellation| {
        let before = interrupt_count();
        let result = run_codex_turn_process_with_policy(
            "run-scenario",
            scenario,
            directory.path(),
            scenario,
            None,
            scenario_policy,
            &channel,
            cancellation,
        );
        assert!(
            interrupt_count().saturating_sub(before) <= 1,
            "scenario sent more than one interrupt: {scenario}"
        );
        result
    };
    for (scenario, expected) in [
        (
            "thread-hang",
            runtime::contracts::RunFailureCode::RequestTimeout,
        ),
        (
            "turn-hang",
            runtime::contracts::RunFailureCode::RequestTimeout,
        ),
        (
            "invalid-thread-id",
            runtime::contracts::RunFailureCode::ProtocolError,
        ),
        (
            "invalid-turn-id",
            runtime::contracts::RunFailureCode::ProtocolError,
        ),
        (
            "progress-hang",
            runtime::contracts::RunFailureCode::ProgressTimeout,
        ),
        (
            "terminal-hang",
            runtime::contracts::RunFailureCode::TerminalTimeout,
        ),
        (
            "malformed",
            runtime::contracts::RunFailureCode::ProtocolError,
        ),
        (
            "provider-error",
            runtime::contracts::RunFailureCode::ProviderError,
        ),
        (
            "terminal-failed",
            runtime::contracts::RunFailureCode::ProviderError,
        ),
        (
            "approval",
            runtime::contracts::RunFailureCode::PolicyViolation,
        ),
        (
            "child-exit",
            runtime::contracts::RunFailureCode::ChildExited,
        ),
        (
            "foreign",
            runtime::contracts::RunFailureCode::ProgressTimeout,
        ),
        (
            "response-too-large",
            runtime::contracts::RunFailureCode::ResponseTooLarge,
        ),
        (
            "stream-too-large",
            runtime::contracts::RunFailureCode::ResponseTooLarge,
        ),
    ] {
        // Protocol rejection tests must not accidentally assert Python startup latency.
        // Keep the short deadlines only for scenarios explicitly testing timeout behavior.
        let scenario_policy = if matches!(
            expected,
            runtime::contracts::RunFailureCode::RequestTimeout
                | runtime::contracts::RunFailureCode::ProgressTimeout
                | runtime::contracts::RunFailureCode::TerminalTimeout
        ) {
            policy
        } else {
            runtime::contracts::RunSupervisionPolicy {
                request_timeout_ms: 5_000,
                progress_idle_timeout_ms: 5_000,
                terminal_gap_timeout_ms: 5_000,
                hard_timeout_ms: 10_000,
                ..policy
            }
        };
        let failure = run_scenario(scenario, scenario_policy, &RunCancellation::default())
            .expect_err("scenario must fail");
        assert_eq!(failure.code, expected, "scenario: {scenario}");
        assert!(!failure.message.contains("SAAA_PRIVATE_"));
    }
    let hard_policy = runtime::contracts::RunSupervisionPolicy {
        progress_idle_timeout_ms: 200,
        hard_timeout_ms: 30,
        ..policy
    };
    assert_eq!(
        run_scenario("hard-hang", hard_policy, &RunCancellation::default())
            .expect_err("hard timeout must fail")
            .code,
        runtime::contracts::RunFailureCode::HardTimeout
    );
    for forbidden in ["fileChange", "mcpToolCall", "dynamicToolCall", "webSearch"] {
        assert_eq!(
            run_scenario(forbidden, policy, &RunCancellation::default())
                .expect_err("forbidden item must fail")
                .code,
            runtime::contracts::RunFailureCode::PolicyViolation
        );
    }
    let duplicate = run_scenario("duplicate", policy, &RunCancellation::default())
        .expect("duplicate notifications must not break completion");
    assert_eq!(duplicate.content, "SAAA_DUPLICATE_OK");

    let unresponsive_cancel = Arc::new(RunCancellation::default());
    let cancel_trigger = unresponsive_cancel.clone();
    let cancel_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        cancel_trigger.cancel();
    });
    let unresponsive = run_scenario("cancel-no-response", policy, &unresponsive_cancel)
        .expect_err("unresponsive cancellation must finish");
    cancel_thread.join().expect("cancel trigger joins");
    assert_eq!(
        unresponsive.code,
        runtime::contracts::RunFailureCode::UserCancelled
    );
    let interrupts_before_atomic = interrupt_count();

    let mut connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
        .execute(
            "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
                 VALUES('coding-atomic','Atomic','coding','1','1')",
            [],
        )
        .expect("coding conversation inserts");
    let mut documents = default_settings_input();
    let codex = documents
        .iter_mut()
        .find(|document| document.namespace == "providers.agent")
        .expect("Codex settings exist");
    codex.value_json["enabled"] = Value::Bool(true);
    codex.value_json["model"] = Value::String("normal".to_string());
    save_settings_documents_to_connection(&mut connection, &documents)
        .expect("Codex settings save");
    let state = app_state(connection);
    let committed_terminal_states =
        Arc::new(Mutex::new(
            Vec::<(String, String, String, Option<String>)>::new(),
        ));
    let committed_terminal_states_for_channel = committed_terminal_states.clone();
    let database_for_channel = state.sqlite_writer.clone();
    let atomic_channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            let event: Value = serde_json::from_str(&value).expect("runtime event decodes");
            let event_type = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if matches!(event_type, "messageCompleted" | "cancelled" | "failed") {
                let run_id = event
                    .get("runId")
                    .and_then(Value::as_str)
                    .expect("terminal event has run id");
                let database = database_for_channel.lock().expect("database lock");
                let (status, failure_code): (String, Option<String>) = database
                    .query_row(
                        "SELECT status,failure_code FROM runtime_runs WHERE id=?1",
                        [run_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .expect("committed run reads from terminal callback");
                committed_terminal_states_for_channel
                    .lock()
                    .expect("terminal state lock")
                    .push((
                        run_id.to_string(),
                        event_type.to_string(),
                        status,
                        failure_code,
                    ));
            }
        }
        Ok(())
    });
    let atomic_input = StartTurnInput {
        run_id: "run-atomic".to_string(),
        conversation_id: "coding-atomic".to_string(),
        content: "ATOMIC".to_string(),
        workspace_path: Some(directory.path().to_string_lossy().into_owned()),
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    tauri::async_runtime::block_on(execute_turn(
        &state,
        &atomic_input,
        &atomic_channel,
        Arc::new(RunCancellation::default()),
        Some(policy),
    ))
    .expect("atomic Codex turn succeeds");
    assert_eq!(interrupt_count(), interrupts_before_atomic);
    assert_eq!(
        committed_terminal_states
            .lock()
            .expect("terminal state lock")
            .as_slice(),
        [(
            "run-atomic".to_string(),
            "messageCompleted".to_string(),
            "completed".to_string(),
            None
        )]
    );
    let database = state.sqlite_writer.lock().expect("database lock");
    let (status, supervisor_version, assistant_count): (String, String, i64) = database
        .query_row(
            "SELECT r.status,r.supervisor_version,
                        (SELECT COUNT(*) FROM conversation_messages
                         WHERE conversation_id='coding-atomic' AND role='assistant')
                 FROM runtime_runs r WHERE r.id='run-atomic'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("atomic state reads");
    assert_eq!(status, "completed");
    assert_eq!(supervisor_version, runtime::contracts::SUPERVISOR_VERSION);
    assert_eq!(assistant_count, 1);
    drop(database);

    let run_atomic_scenario = |run_id: &str, scenario: &str, cancellation: Arc<RunCancellation>| {
        {
            let mut database = state.sqlite_writer.lock().expect("database lock");
            let mut documents = default_settings_input();
            let codex = documents
                .iter_mut()
                .find(|document| document.namespace == "providers.agent")
                .expect("Codex settings exist");
            codex.value_json["enabled"] = Value::Bool(true);
            codex.value_json["model"] = Value::String(scenario.to_string());
            save_settings_documents_to_connection(&mut database, &documents)
                .expect("scenario Codex settings save");
        }
        let input = StartTurnInput {
            run_id: run_id.to_string(),
            conversation_id: "coding-atomic".to_string(),
            content: scenario.to_string(),
            workspace_path: Some(directory.path().to_string_lossy().into_owned()),
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        tauri::async_runtime::block_on(execute_turn(
            &state,
            &input,
            &atomic_channel,
            cancellation,
            Some(policy),
        ))
    };

    let cancellation = Arc::new(RunCancellation::default());
    let cancel_trigger = cancellation.clone();
    let cancel_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        cancel_trigger.cancel();
    });
    let interrupts_before_cancel = interrupt_count();
    let cancelled = run_atomic_scenario("run-atomic-cancel", "cancel-no-response", cancellation)
        .expect_err("cancel scenario must cancel");
    cancel_thread.join().expect("atomic cancel trigger joins");
    assert!(interrupt_count().saturating_sub(interrupts_before_cancel) <= 1);
    assert_eq!(
        cancelled.code,
        runtime::contracts::RunFailureCode::UserCancelled
    );
    let interrupts_before_progress = interrupt_count();
    let progress = run_atomic_scenario(
        "run-atomic-progress",
        "progress-hang",
        Arc::new(RunCancellation::default()),
    )
    .expect_err("progress scenario must time out");
    assert_eq!(
        progress.code,
        runtime::contracts::RunFailureCode::ProgressTimeout
    );
    assert_eq!(interrupt_count() - interrupts_before_progress, 1);
    let interrupts_before_policy = interrupt_count();
    let policy_violation = run_atomic_scenario(
        "run-atomic-policy",
        "fileChange",
        Arc::new(RunCancellation::default()),
    )
    .expect_err("policy scenario must fail");
    assert_eq!(
        policy_violation.code,
        runtime::contracts::RunFailureCode::PolicyViolation
    );
    assert_eq!(interrupt_count() - interrupts_before_policy, 1);

    let terminal_states = committed_terminal_states
        .lock()
        .expect("terminal state lock");
    for expected in [
        (
            "run-atomic-cancel",
            "cancelled",
            "cancelled",
            Some("user-cancelled"),
        ),
        (
            "run-atomic-progress",
            "failed",
            "failed",
            Some("progress-timeout"),
        ),
        (
            "run-atomic-policy",
            "failed",
            "failed",
            Some("policy-violation"),
        ),
    ] {
        let matching = terminal_states
            .iter()
            .filter(|entry| entry.0 == expected.0)
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1, "one terminal event for {}", expected.0);
        assert_eq!(matching[0].1, expected.1);
        assert_eq!(matching[0].2, expected.2);
        assert_eq!(matching[0].3.as_deref(), expected.3);
    }
    drop(terminal_states);
    assert!(cancellation_result.is_err());
    let log = fs::read_to_string(log_path).expect("fixture log loads");
    assert!(log.contains("thread/start"));
    assert!(log.contains("thread/resume"));
    assert!(log.contains("turn/interrupt"));
    assert!(log.contains("read-only"));
    assert!(log.contains("\"approvalPolicy\": \"never\""));
    assert!(log.contains("\"network_access\": false"));
    assert!(received
        .lock()
        .expect("event lock")
        .iter()
        .any(|event| { event.contains("SAAA_OK") && event.contains("\"type\":\"delta\"") }));
}

#[cfg(not(coverage))]
#[test]
#[ignore = "requires a local Codex runtime, authentication, and network access"]
fn codex_live_read_only_turn_completes() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let outcome = run_codex_turn_process(
        "run-live-smoke",
        "Reply with exactly SAAA_LIVE_OK. Do not use tools.",
        workspace.path(),
        "",
        None,
        120_000,
        &events,
        &RunCancellation::default(),
    )
    .expect("live Codex turn succeeds");
    assert!(outcome.content.contains("SAAA_LIVE_OK"));
    assert_eq!(
        fs::read_dir(workspace.path())
            .expect("workspace remains readable")
            .count(),
        0,
        "read-only Codex turn must not create workspace files"
    );
}

#[cfg(not(coverage))]
#[test]
#[ignore = "requires a local Codex runtime, authentication, and network access"]
fn codex_live_read_only_turn_cancels_after_turn_start() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let cancellation = Arc::new(RunCancellation::default());
    let cancellation_for_events = cancellation.clone();
    let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |_| {
        cancellation_for_events.cancel();
        Ok(())
    });
    let failure = run_codex_turn_process(
        "run-live-cancel",
        "Explain the read-only runtime lifecycle in detail. Do not use tools.",
        workspace.path(),
        "",
        None,
        120_000,
        &events,
        &cancellation,
    )
    .expect_err("live Codex turn is cancelled after turn/start");
    assert_eq!(
        failure.code,
        runtime::contracts::RunFailureCode::UserCancelled
    );
    assert_eq!(
        fs::read_dir(workspace.path())
            .expect("workspace remains readable")
            .count(),
        0,
        "cancelled read-only Codex turn must not create workspace files"
    );
}

fn world_body_history(conversation_id: &str) -> Vec<ConversationMessage> {
    vec![
        ConversationMessage {
            parts: None,
            id: "context-system".into(),
            conversation_id: conversation_id.into(),
            role: "system".into(),
            content: "policy".into(),
            created_at: "system".into(),
        },
        ConversationMessage {
            parts: None,
            id: "context-world".into(),
            conversation_id: conversation_id.into(),
            role: "assistant".into(),
            content: "WORLD_BLOCK_PRESENT".into(),
            created_at: "1".into(),
        },
        ConversationMessage {
            parts: None,
            id: "context-current".into(),
            conversation_id: conversation_id.into(),
            role: "user".into(),
            content: "hello".into(),
            created_at: "2".into(),
        },
    ]
}

async fn run_world_body_case(valid: bool, run_id: &str, session_provider_id: &str) -> (Value, i64) {
    let (endpoint, captures, server) =
        spawn_llm_http_fixture(vec![LlmHttpStep::Delta("ok"), LlmHttpStep::Complete]).await;
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let input = StartTurnInput {
        run_id: run_id.to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "hello".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    prepare_runtime_run(&state, &input).expect("runtime prepares");
    let session_id = begin_test_provider_session(
        &state,
        &input.run_id,
        session_provider_id,
        "openai-compatible",
    )
    .expect("provider session starts");
    let world = crate::runtime::context::world::turn::WorldLive::for_test(
        valid,
        "WORLD_BLOCK_PRESENT",
        Some("WORLD_BLOCK_ABSENT"),
    );
    let history = world_body_history(&input.conversation_id);
    // The record must list the World source exactly when the body carries its block.
    let world_candidate = crate::runtime::context::source::Candidate::untrusted(
        "world-candidate".to_string(),
        crate::runtime::context::world::source::WORLD_KIND,
        Vec::new(),
        crate::runtime::context::source::Requirement::May,
        "world-source".to_string(),
        1,
        0,
        "world".to_string(),
    );
    let context_sources = std::slice::from_ref(&world_candidate);
    let provider = OpenAiCompatibleProviderSettings {
        endpoint,
        ..direct_provider(session_provider_id, "local")
    };
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let outcome = stream_model_provider(
        &provider,
        &history,
        5_000,
        ModelStreamContext {
            reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &channel,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources,
            context_omissions: &[],
            output_persistence: Some(ProviderOutputPersistence {
                state: &state,
                session_id: &session_id,
                world: Some(&world),
            }),
        },
    )
    .await;
    server.await.expect("fixture joins");
    let ProviderAttemptOutcome::Completed { .. } = outcome else {
        panic!("provider stream should complete");
    };
    let captures = captures.lock().expect("capture lock");
    assert_eq!(captures.len(), 1, "one provider request");
    let body = serde_json::from_str(&captures[0]).expect("request JSON");
    drop(captures);
    let selected_world: i64 = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT COALESCE(MAX(selected),0) FROM context_generation_inputs \
                     WHERE source_kind=?1",
                    [crate::runtime::context::world::source::WORLD_KIND],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .expect("world input record reads");
    (body, selected_world)
}

#[tokio::test]
async fn expired_world_block_is_removed_from_the_sent_provider_body_and_record() {
    let (body, selected_world) =
        run_world_body_case(false, "run-world-expired", "world-expired-fixture").await;
    let rendered = body["messages"].to_string();
    assert!(
        !rendered.contains("WORLD_BLOCK_PRESENT"),
        "an expired World block must not reach the provider body"
    );
    assert!(
        rendered.contains("WORLD_BLOCK_ABSENT"),
        "the World-free rendering must reach the provider body"
    );
    assert_eq!(
        selected_world, 0,
        "an expired World must not be recorded as selected"
    );
}

#[tokio::test]
async fn valid_world_block_is_sent_to_the_provider_body_and_record() {
    let (body, selected_world) =
        run_world_body_case(true, "run-world-valid", "world-valid-fixture").await;
    let rendered = body["messages"].to_string();
    assert!(
        rendered.contains("WORLD_BLOCK_PRESENT"),
        "a current World block must reach the provider body"
    );
    assert!(
        !rendered.contains("WORLD_BLOCK_ABSENT"),
        "the World-free rendering must not replace a current World block"
    );
    assert_eq!(
        selected_world, 1,
        "a current World must be recorded as selected"
    );
}

fn two_step_role_policy(front_provider: &str, reason_provider: &str) -> Value {
    json!({
        "schemaVersion": 1, "enabled": true,
        "actors": [{
            "id":"front","label":"Front","aliases":[],"transport":"provider",
            "providerId":front_provider,"model":null,"location":"local",
            "resourceGroup":"front","maxInputBytes":16384,"capabilities":["reason"]
        }, {
            "id":"reason","label":"Reason","aliases":[],"transport":"provider",
            "providerId":reason_provider,"model":null,"location":"local",
            "resourceGroup":"reason","maxInputBytes":16384,"capabilities":["reason"]
        }],
        "roles":{"frontend":"front","reasoner":"reason","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},
        "recipes":[{"id":"ack-then-reason","action":"respond","roles":["frontend","reasoner"],"enabled":true}],
        "limits":{"maxReasoningSteps":2,"maxToolCalls":0,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":0,"maxAutomaticSwitches":0,"maxEstimatedCostMicros":null},
        "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
        "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
        "premiumApproval":"never",
        "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false},
        "adaptiveImprovement":{"enabled":false,"providerRecipe":false,"tool":false,"plan":false,"notification":false}
    })
}

#[tokio::test]
async fn rr_05_normal_turn_two_steps_commits_only_the_final_answer() {
    let (front_endpoint, front_requests, front_server) =
        spawn_llm_http_fixture(vec![LlmHttpStep::Delta("ack draft"), LlmHttpStep::Complete]).await;
    let (reason_endpoint, reason_requests, reason_server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta("reasoner final"),
        LlmHttpStep::Complete,
    ])
    .await;
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let mut documents = default_settings_input();
    documents
        .iter_mut()
        .find(|document| document.namespace == "providers.model")
        .expect("provider settings")
        .value_json = json!({
    "harness": { "address": "http://localhost:9810" },
    "providers": [{
        "kind": "openai-compatible", "id": "front-provider", "enabled": true,
        "label": "Front", "location": "local", "endpoint": front_endpoint,
        "model": "front-model", "authentication": "none"
    }, {
        "kind": "openai-compatible", "id": "reason-provider", "enabled": true,
        "label": "Reason", "location": "local", "endpoint": reason_endpoint,
        "model": "reason-model", "authentication": "none"
    }],
        "reasoningEffort": "medium"
    });
    let task_routes = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.tasks")
        .expect("task routes");
    task_routes.value_json["voiceSpeak"]["source"] = json!("harness");
    task_routes.value_json["voiceSpeak"]["providerId"] = Value::Null;
    let role_policy = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.roles")
        .expect("role policy");
    role_policy.value_json = two_step_role_policy("front-provider", "reason-provider");
    save_settings_documents_to_connection(
        &mut state.sqlite_writer.lock().expect("database lock"),
        &documents,
    )
    .expect("settings save");
    let input = StartTurnInput {
        run_id: "rr-two-step-run".into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: "two step request".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_events = events.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            captured_events.lock().expect("events").push(value);
        }
        Ok(())
    });
    execute_turn(
        &state,
        &input,
        &channel,
        Arc::new(RunCancellation::default()),
        None,
    )
    .await
    .expect("two-step turn completes");
    front_server.await.expect("front server");
    reason_server.await.expect("reason server");

    assert_eq!(front_requests.lock().expect("front requests").len(), 1);
    assert_eq!(reason_requests.lock().expect("reason requests").len(), 1);
    let database = state.sqlite_writer.lock().expect("database lock");
    let assistant_messages: Vec<String> = database
        .prepare("SELECT content FROM conversation_messages WHERE conversation_id=?1 AND role='assistant' ORDER BY created_at,id")
        .expect("messages statement")
        .query_map([PRIMARY_CONVERSATION_ID], |row| row.get(0))
        .expect("messages")
        .collect::<Result<Vec<_>, _>>()
        .expect("assistant messages");
    assert_eq!(assistant_messages, vec!["reasoner final"]);
    let step_states = database
        .prepare("SELECT status FROM rr_steps WHERE root_id=?1 ORDER BY ordinal")
        .expect("steps statement")
        .query_map([&input.run_id], |row| row.get::<_, String>(0))
        .expect("steps")
        .collect::<Result<Vec<_>, _>>()
        .expect("step states");
    assert_eq!(step_states, vec!["succeeded", "succeeded"]);
    assert_eq!(
        database
            .query_row(
                "SELECT count(*) FROM rr_outputs o JOIN rr_steps s ON s.id=o.step_id WHERE s.root_id=?1 AND o.accepted=1",
                [&input.run_id],
                |row| row.get::<_, i64>(0),
            )
            .expect("accepted outputs"),
        1
    );
    drop(database);
    let event_log = events.lock().expect("events").join("\n");
    assert_eq!(
        event_log.matches("\"type\":\"messageCompleted\"").count(),
        1
    );
    assert!(!event_log.contains("ack draft"));
    assert_eq!(event_log.matches("\"type\":\"delta\"").count(), 0);
}

#[tokio::test]
async fn rr_09_partial_role_step_leaks_no_draft_and_cancels_remaining_work() {
    use std::io::ErrorKind;
    use std::net::TcpListener;

    let (front_endpoint, _, front_server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta("private partial draft"),
        LlmHttpStep::Disconnect,
    ])
    .await;
    let reason_listener = TcpListener::bind("127.0.0.1:0").expect("reason listener");
    let reason_address = reason_listener.local_addr().expect("reason address");
    reason_listener
        .set_nonblocking(true)
        .expect("reason nonblocking");
    let reason_hit = Arc::new(AtomicBool::new(false));
    let reason_hit_for_server = reason_hit.clone();
    let reason_server = thread::spawn(move || {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            match reason_listener.accept() {
                Ok(_) => {
                    reason_hit_for_server.store(true, Ordering::SeqCst);
                    return;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let mut documents = default_settings_input();
    documents
        .iter_mut()
        .find(|document| document.namespace == "providers.model")
        .expect("provider settings")
        .value_json = json!({
        "harness":{"address":"http://localhost:9810"},
        "providers":[{
            "kind":"openai-compatible","id":"partial-front","enabled":true,
            "label":"Partial front","location":"local","endpoint":front_endpoint,
            "model":"front-model","authentication":"none"
        }, {
            "kind":"openai-compatible","id":"forbidden-reason","enabled":true,
            "label":"Forbidden reason","location":"local",
            "endpoint":format!("http://{reason_address}/v1"),
            "model":"reason-model","authentication":"none"
        }], "reasoningEffort":"medium"
    });
    let task_routes = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.tasks")
        .expect("task routes");
    task_routes.value_json["voiceSpeak"]["source"] = json!("harness");
    task_routes.value_json["voiceSpeak"]["providerId"] = Value::Null;
    documents
        .iter_mut()
        .find(|document| document.namespace == "routing.roles")
        .expect("role policy")
        .value_json = two_step_role_policy("partial-front", "forbidden-reason");
    save_settings_documents_to_connection(
        &mut state.sqlite_writer.lock().expect("database lock"),
        &documents,
    )
    .expect("settings save");
    let input = StartTurnInput {
        run_id: "rr-partial-step-run".into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: "partial two step request".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_events = events.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            captured_events.lock().expect("events").push(value);
        }
        Ok(())
    });
    execute_turn(
        &state,
        &input,
        &channel,
        Arc::new(RunCancellation::default()),
        None,
    )
    .await
    .expect_err("partial role step fails the turn");
    front_server.await.expect("front server");
    reason_server.join().expect("reason server");
    assert!(!reason_hit.load(Ordering::SeqCst));

    let database = state.sqlite_writer.lock().expect("database lock");
    let root_phase: String = database
        .query_row(
            "SELECT phase FROM rr_roots WHERE root_id=?1",
            [&input.run_id],
            |row| row.get(0),
        )
        .expect("root phase");
    let step_states = database
        .prepare("SELECT status FROM rr_steps WHERE root_id=?1 ORDER BY ordinal")
        .expect("steps statement")
        .query_map([&input.run_id], |row| row.get::<_, String>(0))
        .expect("steps")
        .collect::<Result<Vec<_>, _>>()
        .expect("step states");
    let assistant_count: i64 = database
        .query_row(
            "SELECT count(*) FROM conversation_messages WHERE conversation_id=?1 AND role='assistant'",
            [PRIMARY_CONVERSATION_ID],
            |row| row.get(0),
        )
        .expect("assistant count");
    assert_eq!(root_phase, "failed");
    assert_eq!(step_states, vec!["failed", "interrupted"]);
    assert_eq!(assistant_count, 0);
    drop(database);
    let event_log = events.lock().expect("events").join("\n");
    assert!(!event_log.contains("private partial draft"));
    assert_eq!(event_log.matches("\"type\":\"delta\"").count(), 0);
    assert_eq!(
        event_log.matches("\"type\":\"messageCompleted\"").count(),
        0
    );
}
