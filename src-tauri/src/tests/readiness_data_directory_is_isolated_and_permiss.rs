use super::*;
#[test]
pub(super) fn readiness_data_directory_is_isolated_and_permission_bounded() {
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
pub(super) async fn openai_compatible_stream_fixture_projects_deltas() {
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
    let content = diagnostic_stream_with_key(
        &provider,
        &history,
        5_000,
        Some("ephemeral-connection-token"),
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
pub(super) async fn http_disconnect_preserves_partial_output_without_regeneration() {
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
    let outcome = diagnostic_stream(
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
pub(super) async fn dynamic_lan_stream_policy_requires_sse_for_stream_requests() {
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
    let outcome = diagnostic_stream_with_key(
        &provider,
        &history,
        5_000,
        Some("ephemeral-connection-token"),
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
            kind: ProviderFailureKind::Protocol
                | ProviderFailureKind::ResponseInterrupted
                | ProviderFailureKind::Network,
            output_started: false,
            ..
        }
    ));
}
#[test]
pub(super) fn current_recall_offer_executes_and_records_only_the_authorized_history() {
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
        run_id: "run_recall-tool".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "前の話を思い出して".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    crate::test_support::prepare_user_turn(&state, &input).expect("runtime prepares");
    let session_id =
        begin_test_provider_session(&state, &input.run_id, "recall-fixture", "openai-compatible")
            .expect("provider session starts");
    let persistence = Some(ProviderOutputPersistence {
        state: &state,
        session_id: &session_id,
        world: None,
    });
    let offer = available_agent_tools(persistence, &input, 0, 0, 0);
    assert!(tool_was_offered(&offer.definitions, "recall_conversation"));
    let result = execute_recall_tool(
        persistence,
        &input,
        &runtime::agent_tools::AgentToolCall {
            id: "call_recall_1".into(),
            name: "recall_conversation".into(),
            arguments: json!({"query":"SQLite"}).to_string(),
        },
    );
    assert!(result.contains("SQLite の検索方式"));
    assert!(!result.contains("前の話を思い出して"));
    let receipts: i64 = state
        .sqlite_readers
        .read(|c| {
            c.query_row(
                "SELECT count(*) FROM conversation_recall_receipts WHERE runtime_run_id=?1",
                [&input.run_id],
                |r| r.get(0),
            )
            .map_err(database_error)
        })
        .unwrap();
    assert_eq!(receipts, 1);
}
#[test]
pub(super) fn voice_policy_tool_quota_is_independent_from_other_agent_tools() {
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

async fn diagnostic_stream(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout: u64,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    diagnostic_stream_with_key(provider, history, timeout, None, context).await
}
async fn diagnostic_stream_with_key(
    provider: &OpenAiCompatibleProviderSettings,
    history: &[ConversationMessage],
    timeout: u64,
    key: Option<&str>,
    context: ModelStreamContext<'_>,
) -> ProviderAttemptOutcome {
    let authorization = key.map(|key| format!("Bearer {key}"));
    let result = providers::chat_completions::run_with_options(
        &provider.endpoint,
        authorization.as_deref(),
        &provider.model,
        history,
        timeout,
        context,
        providers::chat_completions::RequestMode::Stream,
        &saaa_larm_session::http_api::LlmOptions {
            tools: false,
            ..Default::default()
        },
    )
    .await;
    let cleanup = CleanupOutcome::NotApplicable;
    match result {
        Ok(content) => ProviderAttemptOutcome::Completed { content, cleanup },
        Err(ProviderAttemptError::Cancelled { output_started }) => {
            ProviderAttemptOutcome::Cancelled {
                output_started,
                cleanup,
            }
        }
        Err(ProviderAttemptError::Failed {
            kind,
            output_started,
            ..
        }) => ProviderAttemptOutcome::Failed {
            kind,
            output_started,
            public_message: kind.public_message(),
            cleanup,
        },
    }
}
