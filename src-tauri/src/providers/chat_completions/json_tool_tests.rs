use super::*;

#[tokio::test]
async fn json_completion_executes_offered_tools_and_returns_their_result() {
    let c = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&c).unwrap();
    c.execute(
        "UPDATE coding_settings SET value_json=json_set(value_json,'$.enabled',json('true'))",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO conversation_messages VALUES('http-source',?1,'user','inspect','1')",
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
    let first=json!({"model":"fixture","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"inspect-call","type":"function","function":{"name":"coding_inspect","arguments":"{\"jobId\":\"missing\"}"}}]},"finish_reason":"tool_calls"}]}).to_string();
    let last=json!({"model":"fixture","choices":[{"index":0,"message":{"role":"assistant","content":"Job unavailable"},"finish_reason":"stop"}]}).to_string();
    let (endpoint, server) = fixture(vec![(200, first, 0), (200, last, 0)]).await;
    let mut input = input();
    input.conversation_id = crate::PRIMARY_CONVERSATION_ID.into();
    let output = run_mode(
        &endpoint,
        None,
        "fixture",
        &[],
        5000,
        ModelStreamContext {
            reasoning_effort: "low",
            max_output_tokens: 64,
            input: &input,
            on_event: &Sink::default(),
            cancellation: Arc::default(),
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &state,
                session_id: &session,
            }),
        },
        RequestMode::JsonTools,
    )
    .await
    .unwrap();
    assert_eq!(output, "Job unavailable");
    let requests = server.await.unwrap();
    assert!(requests.iter().all(|r| r["stream"] == false));
    assert!(requests[0].get("reasoning_effort").is_none());
    assert_eq!(requests[0]["parallel_tool_calls"], false);
    assert!(requests[0].get("chat_template_kwargs").is_none());
    assert!(requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["function"]["name"] == "coding_inspect"));
    let tool = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap();
    assert_eq!(tool["tool_call_id"], "inspect-call");
    let result: Value = serde_json::from_str(tool["content"].as_str().unwrap()).unwrap();
    assert!(result.get("error").is_some());
}
