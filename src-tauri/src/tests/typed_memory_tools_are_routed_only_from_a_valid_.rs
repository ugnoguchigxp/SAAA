use super::*;
#[tokio::test]
pub(super) async fn typed_memory_tools_are_routed_only_from_a_valid_typed_manifest() {
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
            "continue_work",
            "read_record",
            "recall_activity",
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
        None,
    )
    .await;
    assert!(error.contains("invalid-memory-input"));
    assert!(!error.contains("forbidden"));
}
#[tokio::test]
pub(super) async fn typed_memory_execution_cannot_exceed_the_provider_deadline() {
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
        None,
    );
    let error = tokio::time::timeout(Duration::from_millis(250), execution)
        .await
        .expect("typed recall respects the provider deadline");
    assert!(error.contains("typed-memory-unavailable"));

    release_sender.send(()).expect("fixture releases");
    server.join().expect("fixture joins");
}
#[test]
pub(super) fn malformed_recall_calls_consume_the_persistent_turn_limit() {
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
pub(super) async fn recall_tool_rounds_share_one_provider_timeout_budget() {
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
pub(super) async fn provider_stream_stops_when_the_tauri_consumer_disconnects() {
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
pub(super) async fn model_provider_redirects_are_not_followed() {
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
            kind: ProviderFailureKind::Contract
                | ProviderFailureKind::Connect
                | ProviderFailureKind::Network,
            output_started: false,
            ..
        }
    ));
    assert!(!target_hit.load(Ordering::SeqCst));
}
