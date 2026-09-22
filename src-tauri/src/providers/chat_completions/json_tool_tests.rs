#![cfg(test)]

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
    let history = [ConversationMessage {
        parts: None,
        id: "http-source".into(),
        conversation_id: input.conversation_id.clone(),
        role: "user".into(),
        content: input.content.clone(),
        created_at: "1".into(),
    }];
    let output = run_mode(
        &endpoint,
        None,
        "fixture",
        &history,
        5000,
        ModelStreamContext {
            reasoning_effort: "low",
            max_output_tokens: 64,
            input: &input,
            on_event: &Sink::default(),
            cancellation: Arc::default(),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &state,
                session_id: &session,
                world: None,
            }),
        },
        RequestMode::JsonTools,
    )
    .await
    .unwrap();
    assert_eq!(output, "Job unavailable");
    crate::runtime::context::generation::assert_two_round_tool_manifest(&state, "http_fixture");
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
    let tool_audit = state
        .sqlite_readers
        .read(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT event_name,json_extract(attributes_json,'$.toolName'),
                            json_extract(attributes_json,'$.durationMs')
                     FROM audit_events
                     WHERE runtime_run_id='http_fixture'
                       AND event_name IN ('tool-execution-started','tool-execution-finished')
                     ORDER BY sequence",
                )
                .map_err(crate::database_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<u64>>(2)?,
                    ))
                })
                .map_err(crate::database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(crate::database_error)?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(tool_audit.len(), 2);
    assert_eq!(tool_audit[0].0, "tool-execution-started");
    assert_eq!(tool_audit[0].1, "coding_inspect");
    assert_eq!(tool_audit[1].0, "tool-execution-finished");
    assert_eq!(tool_audit[1].1, "coding_inspect");
    assert!(tool_audit[1].2.is_some());
}
