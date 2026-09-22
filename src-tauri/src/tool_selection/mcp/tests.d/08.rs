#[tokio::test]
async fn a15_redirect_is_not_followed() {
    let target_state = Arc::new(ServerState::default());
    let target = MockServer::start(target_state.clone()).await;
    let (redirect_url, redirect_task) = redirect_server(target.url()).await;

    let connection = Connection::open_in_memory().expect("in-memory");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let writer = Arc::new(SqliteWriter::from_connection(connection));
    let principal = crate::tool_selection::service::ensure_principal(&writer).expect("principal");
    let manager = McpManager::new(
        writer,
        principal,
        None,
        McpSources {
            sources: vec![McpSourceSpec {
                id: "mcp-redirect".to_string(),
                url: redirect_url,
                enabled: true,
                bearer_token_env: None,
                grants: vec![],
            }],
        },
        Some(hash_embedding()),
        None,
    );
    let error = manager
        .sync_source("mcp-redirect")
        .await
        .expect_err("a 302 must fail the sync");
    assert!(!error.code.is_empty());
    assert_eq!(
        target_state.call_count.load(Ordering::SeqCst),
        0,
        "the redirect target must never be contacted"
    );
    redirect_task.abort();
}
async fn mock_chat_provider(body: String) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let endpoint = format!("http://{}/proxy/v1", listener.local_addr().expect("addr"));
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buffer = vec![0_u8; 16 * 1024];
        let _ = socket.read(&mut buffer).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = socket.write_all(response.as_bytes()).await;
    });
    (endpoint, task)
}
#[tokio::test]
async fn p04_natural_correction_via_conversation_provider_reaches_mcp_search() {
    let state = default_state();
    let mock = MockServer::start(state).await;
    let connection = Connection::open_in_memory().expect("in-memory");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let writer = Arc::new(SqliteWriter::from_connection(connection));
    writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT OR IGNORE INTO conversations(id, title, task_mode, created_at, updated_at)
                     VALUES ('conversation-d4', 'D4', 'conversation', '1', '1')",
                    [],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("conversation");
    let principal = crate::tool_selection::service::ensure_principal(&writer).expect("principal");
    let manager = McpManager::new(
        writer.clone(),
        principal.clone(),
        None,
        McpSources {
            sources: vec![McpSourceSpec {
                id: "mcp-test".to_string(),
                url: mock.url(),
                enabled: true,
                bearer_token_env: None,
                grants: vec![user_grant("search"), user_grant("other")],
            }],
        },
        Some(hash_embedding()),
        None,
    );
    manager.sync_source("mcp-test").await.expect("sync");

    // The conversation provider is a local mock that returns the extraction the model would.
    let extraction = json!({
        "scenario": {
            "intent": "search instead of other",
            "operation": "search",
            "objectType": "document",
            "phase": "discover",
            "inputKind": "text"
        },
        "feedback": [{
            "kind": "tool_choice",
            "decisionId": null,
            "rejectedToolId": "search",
            "preferredToolId": "other",
            "scope": "conversation",
            "duration": "persistent",
            "evidence": { "start": 0, "end": 6, "text": "search" },
            "condition": {
                "operation": "search",
                "objectType": "document",
                "phase": null,
                "inputKind": null
            }
        }]
    })
    .to_string();
    let chat = json!({
        "model": "fixture",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": extraction },
            "finish_reason": "stop"
        }]
    })
    .to_string();
    let (endpoint, provider_task) = mock_chat_provider(chat).await;
    writer
        .write({
            let endpoint = endpoint.clone();
            move |connection| {
                let mut providers = crate::persistence::load_model_providers(connection)?;
                providers.providers.push(crate::ModelProviderSettings::OpenAiCompatible(
                    crate::OpenAiCompatibleProviderSettings {
                        request_options: None,
                        id: "mock-extraction-provider".to_string(),
                        enabled: true,
                        label: "Mock extraction".to_string(),
                        location: "local".to_string(),
                        endpoint,
                        model: "fixture".to_string(),
                        authentication: "none".to_string(),
                    },
                ));
                let providers_json =
                    serde_json::to_string(&providers).map_err(|error| error.to_string())?;
                connection
                    .execute(
                        "UPDATE settings_documents SET value_json = ?1
                          WHERE namespace = 'providers.model' AND key = 'default'",
                        rusqlite::params![providers_json],
                    )
                    .map_err(|error| error.to_string())?;
                let mut routing = crate::persistence::load_routing_settings(connection)?;
                routing.conversation_respond.source = "provider".to_string();
                routing.conversation_respond.primary_provider_id =
                    Some("mock-extraction-provider".to_string());
                let routing_json =
                    serde_json::to_string(&routing).map_err(|error| error.to_string())?;
                connection
                    .execute(
                        "UPDATE settings_documents SET value_json = ?1
                          WHERE namespace = 'routing.tasks' AND key = 'default'",
                        rusqlite::params![routing_json],
                    )
                    .map_err(|error| error.to_string())?;
                connection
                    .execute(
                        "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                         VALUES ('msg-p04', 'conversation-d4', 'user', 'fixture', '1')",
                        [],
                    )
                    .map_err(|error| error.to_string())?;
                Ok(())
            }
        })
        .expect("configure provider");

    let router = Arc::new(BackendRouter::new(
        Arc::new(FixtureBackend::new()),
        Arc::new(McpBackend::new(manager.clone())),
        Arc::new(crate::records::backend::RecordsBackend::new(writer.clone())),
    ));
    let mut service = ToolSelectionService::new(
        writer.clone(),
        hash_embedding(),
        Arc::new(FixedReranker::new(&[])),
        Arc::new(
            crate::tool_selection::provider_extraction::ConversationProviderExtractor::new(
                writer.clone(),
            ),
        ),
        router,
        f64::NEG_INFINITY,
    );
    service.set_mcp_manager(manager);
    service.set_discovery_configured(true);

    let context = RequestContext::new(&principal, "conversation-d4")
        .with_run(Some("run-p04".into()))
        .with_message(Some("msg-p04".into()));
    let outcome = service
        .begin_turn(&context, "searchではなくotherを使って")
        .await;
    assert!(!outcome.degraded, "the provider extraction must succeed");
    provider_task.await.expect("provider task");

    let response = service
        .search(&context, "search notes", 8)
        .await
        .expect("search");
    let other_tool = descriptors::tool_id("mcp-test", "other");
    let applied = service
        .decision_candidates(&response.decision_id)
        .expect("candidates")
        .into_iter()
        .find(|candidate| candidate.tool_id == other_tool)
        .expect("preferred candidate present");
    assert!(
        !applied.rule_ids.is_empty(),
        "the provider-extracted correction must reach the MCP search"
    );
}
