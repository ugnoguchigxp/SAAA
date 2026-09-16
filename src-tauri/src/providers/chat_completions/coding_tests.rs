use super::*;

#[tokio::test]
async fn coding_http_request_includes_host_workspace_reference() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    c.execute(
        "UPDATE coding_settings SET value_json=json_set(value_json,'$.enabled',json('true'))",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO coding_workspaces VALUES('workspace_http',?1,'/tmp/bbs')",
        [crate::PRIMARY_CONVERSATION_ID],
    )
    .unwrap();
    c.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at) VALUES('http_fixture',?1,'conversation.respond','running','1')",[crate::PRIMARY_CONVERSATION_ID]).unwrap();
    let state = crate::test_support::app_state(c);
    let session = crate::begin_provider_session(
        &state,
        "http_fixture",
        "fixture",
        "openai-compatible",
        &"a".repeat(64),
    )
    .unwrap();
    let body = chunk(json!({"content":"ready"}), Value::Null)
        + &chunk(json!({}), json!("stop"))
        + "data: [DONE]\n\n";
    let (endpoint, server) = fixture(vec![(200, body, 0)]).await;
    let mut input = input();
    input.conversation_id = crate::PRIMARY_CONVERSATION_ID.into();
    let history: Vec<ConversationMessage> =
        [("system", "Existing policy"), ("user", "Build a BBS")]
            .into_iter()
            .map(|(role, content)| ConversationMessage {
                parts: None,
                id: role.into(),
                conversation_id: input.conversation_id.clone(),
                role: role.into(),
                content: content.into(),
                created_at: "1".into(),
            })
            .collect();
    run(
        &endpoint,
        None,
        "fixture",
        &history,
        5000,
        ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 64,
            input: &input,
            on_event: &Sink::default(),
            cancellation: Arc::default(),
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &state,
                session_id: &session,
            }),
        },
    )
    .await
    .unwrap();
    let requests = server.await.unwrap();
    assert_eq!(requests[0]["messages"].as_array().unwrap().len(), 2);
    assert_eq!(requests[0]["messages"][0]["role"], "system");
    assert_eq!(requests[0]["messages"][1]["role"], "user");
    assert!(requests[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .starts_with("Existing policy"));
    assert!(requests[0]["messages"].as_array().unwrap().iter().any(
        |m| m["role"] == "system" && m["content"].as_str().unwrap().contains("workspace_http")
    ));
    assert!(requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["function"]["name"] == "coding_start"));
}
