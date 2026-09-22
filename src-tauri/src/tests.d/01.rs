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
    spawn_llm_http_sequence_fixture(vec![steps]).await
}
async fn spawn_llm_http_sequence_fixture(
    requests: Vec<Vec<LlmHttpStep>>,
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
        for steps in requests {
            let mut socket = accept(&listener, &captured).await;
            if let Some(LlmHttpStep::Fail) = steps.first() {
                socket.write_all(b"HTTP/1.1 503 Unavailable\r\nRetry-After: 60\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
                continue;
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
fn provider_transport_ledger_records_phases_without_payloads() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
        .execute(
            "INSERT INTO conversations(id, task_mode, created_at, updated_at)
             VALUES('conversation-transport', 'conversation', '1', '1')",
            [],
        )
        .expect("conversation inserts");
    let state = app_state(connection);
    begin_simple_runtime_run(
        &state,
        "run-transport",
        "conversation-transport",
        "conversation.respond",
        "provider-transport",
    )
    .expect("runtime starts");
    let session_id = begin_test_provider_session(
        &state,
        "run-transport",
        "provider-transport",
        "openai-compatible",
    )
    .expect("provider session starts");
    let persistence = ProviderOutputPersistence {
        state: &state,
        session_id: &session_id,
        world: None,
    };
    persistence.bind_transport(Some("allocation-transport"));
    persistence.record_transport_event(
        "provider-request-transport",
        "prepared",
        "sse",
        "qwen-worker-fast",
        "http://192.0.2.1:9810/v1/chat/completions",
        None,
    );
    persistence.record_transport_event(
        "provider-request-transport",
        "headers-received",
        "sse",
        "qwen-worker-fast",
        "http://192.0.2.1:9810/v1/chat/completions",
        Some("200"),
    );

    let connection = state.sqlite_writer.lock().expect("database lock");
    let (request_id, route_id, allocation_id): (String, String, String) = connection
        .query_row(
            "SELECT request_id,route_id,allocation_id FROM provider_sessions WHERE id=?1",
            [&session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("provider binding reads");
    assert_eq!(request_id, "provider-request-transport");
    assert_eq!(route_id, "allocated-http");
    assert_eq!(allocation_id, "allocation-transport");
    let phases: Vec<String> = connection
        .prepare("SELECT stage FROM provider_transport_events WHERE request_id=?1 ORDER BY rowid")
        .expect("phase query prepares")
        .query_map(["provider-request-transport"], |row| row.get(0))
        .expect("phases query")
        .collect::<Result<_, _>>()
        .expect("phases read");
    assert_eq!(phases, ["prepared", "headers-received"]);
}
