#[tokio::test]
async fn generative_ui_http_tool_round_persists_a_view_without_speaking_dsl() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    c.execute("UPDATE ui_settings SET enabled=1", []).unwrap();
    c.execute(
        "INSERT INTO conversation_messages VALUES('http-source',?1,'user','hello','1')",
        [crate::PRIMARY_CONVERSATION_ID],
    )
    .unwrap();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('http_fixture',?1,'conversation.respond','running','http-source','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    let state = crate::test_support::app_state(c);
    let session = crate::begin_provider_session(
        &state,
        "http_fixture",
        "fixture",
        "openai-compatible",
        &"a".repeat(64),
    )
    .unwrap();
    let arguments=json!({"definition":"root=ModelStatus(\"larm.status\")","summary":"割り当て履歴を表示しました","mode":"live"}).to_string();
    let first = format!(
        "{}{}data: [DONE]\r\n\r\n",
        chunk(
            json!({"tool_calls":[{"index":0,"id":"ui-call","type":"function","function":{"name":"present_ui","arguments":arguments}}]}),
            Value::Null
        ),
        chunk(json!({}), json!("tool_calls"))
    );
    let second = format!(
        "{}{}data: [DONE]\r\n\r\n",
        chunk(json!({"content":"表示しました。"}), Value::Null),
        chunk(json!({}), json!("stop"))
    );
    let (endpoint, server) = fixture(vec![(200, first, 0), (200, second, 0)]).await;
    let mut input = input();
    input.conversation_id = crate::PRIMARY_CONVERSATION_ID.into();
    let history = [ConversationMessage {
        parts: None,
        id: "http-source".into(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: input.content.clone(),
        created_at: "1".into(),
    }];
    let sink = Sink::default();
    let result = run(
        &endpoint,
        None,
        "fixture",
        &history,
        10_000,
        ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 1000,
            input: &input,
            on_event: &sink,
            cancellation: Arc::new(crate::RunCancellation::default()),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &state,
                session_id: &session,
                world: None,
            }),
        },
    )
    .await
    .unwrap();
    assert_eq!(result, "表示しました。");
    assert_eq!(sink.0.lock().unwrap().join(""), "表示しました。");
    let requests = server.await.unwrap();
    assert!(requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["function"]["name"] == "present_ui"));
    let tool_result: Value = serde_json::from_str(
        requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["role"] == "tool")
            .unwrap()["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert!(tool_result["instanceId"].is_string());
    state
        .sqlite_readers
        .read(|c| {
            let page = crate::persistence::list_message_page_from_connection(
                c,
                crate::PRIMARY_CONVERSATION_ID,
                None,
                30,
            )?;
            assert_eq!(page.messages.len(), 2);
            assert!(page.messages.iter().any(|message| message.parts.is_some()));
            Ok(())
        })
        .unwrap();
}
#[tokio::test]
async fn accepts_server_resolved_model_alias_without_fabricating_a_mapping() {
    let body = (chunk(json!({"content":"ready"}), Value::Null)
        + &chunk(json!({}), json!("stop"))
        + "data: [DONE]\n\n")
        .replace("fixture", "Qwen3.8-27B-ROCmFP4-FAST.gguf");
    let (endpoint, server) = fixture(vec![(200, body, 0)]).await;
    assert_eq!(
        invoke(&endpoint, &Sink::default(), Arc::default())
            .await
            .unwrap(),
        "ready"
    );
    server.await.unwrap();
}

async fn usage_state() -> (crate::AppState, String) {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&connection).unwrap();
    connection
        .execute(
            "INSERT INTO conversation_messages VALUES('http-source',?1,'user','hello','1')",
            [crate::PRIMARY_CONVERSATION_ID],
        )
        .unwrap();
    connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('http_fixture',?1,'conversation.respond','running','http-source','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    let state = crate::test_support::app_state(connection);
    let session = crate::begin_provider_session(
        &state,
        "http_fixture",
        "fixture",
        "openai-compatible",
        &"a".repeat(64),
    )
    .unwrap();
    (state, session)
}

fn usage_context<'a>(
    state: &'a crate::AppState,
    session: &'a str,
    input: &'a crate::StartTurnInput,
    sink: &'a Sink,
) -> ModelStreamContext<'a> {
    ModelStreamContext {
        reasoning_effort: "provider-default",
        max_output_tokens: 1000,
        input,
        on_event: sink,
        cancellation: Arc::new(crate::RunCancellation::default()),
        context_health: "green",
        context_sources: &[],
        context_omissions: &[],
        output_persistence: Some(crate::ProviderOutputPersistence {
            state,
            session_id: session,
            world: None,
        }),
    }
}

#[tokio::test]
async fn cw_13_usage_row_written_on_success() {
    let (state, session) = usage_state().await;
    let body = format!(
        "{}{}data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":12,\"prompt_tokens_details\":{{\"cached_tokens\":3}},\"completion_tokens\":4}}}}\n\ndata: [DONE]\n\n",
        chunk(json!({"content": "ok"}), Value::Null),
        chunk(json!({}), json!("stop"))
    );
    let (endpoint, server) = fixture(vec![(200, body, 0)]).await;
    let mut input = input();
    input.conversation_id = crate::PRIMARY_CONVERSATION_ID.into();
    let history = [ConversationMessage {
        parts: None,
        id: "http-source".into(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: input.content.clone(),
        created_at: "1".into(),
    }];
    let sink = Sink::default();
    let result = run(
        &endpoint,
        None,
        "fixture",
        &history,
        10_000,
        usage_context(&state, &session, &input, &sink),
    )
    .await
    .unwrap();
    assert_eq!(result, "ok");
    server.await.unwrap();
    state
        .sqlite_readers
        .read(|connection| {
            let (input_tokens, cache, source): (i64, i64, String) = connection
                .query_row(
                    "SELECT input_tokens, cache_read_tokens, usage_source FROM generation_usage",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|error| error.to_string())?;
            assert_eq!((input_tokens, cache, source.as_str()), (12, 3, "provider"));
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn cw_13_usage_missing_on_disconnect() {
    let (state, session) = usage_state().await;
    let body = chunk(json!({"content": "partial"}), Value::Null);
    let (endpoint, server) = fixture(vec![(200, body, 0)]).await;
    let mut input = input();
    input.conversation_id = crate::PRIMARY_CONVERSATION_ID.into();
    let history = [ConversationMessage {
        parts: None,
        id: "http-source".into(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: input.content.clone(),
        created_at: "1".into(),
    }];
    let sink = Sink::default();
    let error = run(
        &endpoint,
        None,
        "fixture",
        &history,
        10_000,
        usage_context(&state, &session, &input, &sink),
    )
    .await
    .unwrap_err();
    assert!(format!("{error:?}").to_lowercase().contains("interrupt") || format!("{error:?}").contains("Response"));
    let _ = server.await;
    state
        .sqlite_readers
        .read(|connection| {
            let source: String = connection
                .query_row(
                    "SELECT usage_source FROM generation_usage",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            assert_eq!(source, "disconnected");
            Ok(())
        })
        .unwrap();
}
