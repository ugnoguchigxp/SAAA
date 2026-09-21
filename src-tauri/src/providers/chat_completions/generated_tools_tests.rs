//! C01-C03: generated tools through the real Chat Completions HTTP path.

use super::*;
use crate::generated_capabilities::publication::{tool_name, GeneratedToolsConfig};
use crate::generated_capabilities::tests::{TestEnv, ACCEPTANCE_A, CANDIDATE_A};

pub(super) async fn ready_state() -> (TestEnv, crate::AppState, String, String) {
    let env = TestEnv::start(true);
    let revision = env.ready(CANDIDATE_A, ACCEPTANCE_A).await;
    let name = tool_name(&revision.revision_id).expect("revision id is a uuid");
    let mut state =
        crate::test_state::app_state_with_capabilities(env.writer.clone(), env.service.clone());
    state.generated_tools = GeneratedToolsConfig {
        enabled: true,
        capability_ids: vec![revision.capability_id.clone()],
        diagnostic: None,
    };
    (env, state, revision.revision_id.clone(), name)
}

pub(super) fn seeded_state(env: &TestEnv, state: &crate::AppState, run_id: &str) -> String {
    env.writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO conversation_messages VALUES('m2a-source',?1,'user','hello','1')",
                    [crate::PRIMARY_CONVERSATION_ID],
                )
                .map_err(|error| error.to_string())?;
            connection
                .execute(
                    "INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) \
                     VALUES(?1,?2,'conversation.respond','running','m2a-source','1')",
                    rusqlite::params![run_id, crate::PRIMARY_CONVERSATION_ID],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("run rows insert");
    crate::begin_provider_session(
        state,
        run_id,
        "fixture",
        "openai-compatible",
        &"a".repeat(64),
    )
    .expect("provider session begins")
}

pub(super) fn turn_input(run_id: &str) -> crate::StartTurnInput {
    crate::StartTurnInput {
        run_id: run_id.to_string(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.to_string(),
        content: "hello".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    }
}

pub(super) fn history() -> [ConversationMessage; 1] {
    [ConversationMessage {
        parts: None,
        id: "m2a-source".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        role: "user".into(),
        content: "hello".into(),
        created_at: "1".into(),
    }]
}

pub(super) fn tool_call_stream(name: &str, arguments: &str) -> String {
    format!(
        "{}{}data: [DONE]\r\n\r\n",
        chunk(
            json!({"tool_calls":[{"index":0,"id":"provider-call-id","type":"function",
                "function":{"name":name,"arguments":arguments}}]}),
            Value::Null
        ),
        chunk(json!({}), json!("tool_calls"))
    )
}

fn content_stream(text: &str) -> String {
    format!(
        "{}{}data: [DONE]\r\n\r\n",
        chunk(json!({"content": text}), Value::Null),
        chunk(json!({}), json!("stop"))
    )
}

fn tool_message_content(requests: &[Value]) -> Value {
    let content = requests[1]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("tool message")["content"]
        .as_str()
        .expect("tool content");
    serde_json::from_str(content).expect("tool content is JSON")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c01_generated_definition_is_offered_and_its_result_returns_to_the_conversation() {
    let (env, state, revision_id, name) = ready_state().await;
    let run_id = "m2a-round";
    let session = seeded_state(&env, &state, run_id);
    let arguments = json!({ "enabled": true, "suspended": false }).to_string();
    let (endpoint, server) = fixture(vec![
        (200, tool_call_stream(&name, &arguments), 0),
        (200, content_stream("実行しました。"), 0),
    ])
    .await;
    let input = turn_input(run_id);
    let result = run(
        &endpoint,
        None,
        "fixture",
        &history(),
        10_000,
        ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 1000,
            input: &input,
            on_event: &Sink::default(),
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
    .expect("provider round completes");
    assert_eq!(result, "実行しました。");

    let requests = server.await.unwrap();
    // C01: the definition is in the actual request body, named from the revision.
    assert!(requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["function"]["name"] == name));
    // C01: the tool result is correlated by the provider call id and carries the boolean value.
    let content = tool_message_content(&requests);
    assert_eq!(content["ok"], true, "{content}");
    assert_eq!(content["value"], true, "{content}");
    assert_eq!(content["revisionId"], revision_id);
    assert_ne!(content["callId"], "provider-call-id");
    let request_tool_call = &requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert_eq!(request_tool_call["tool_call_id"], "provider-call-id");

    // The durable call records the conversation origin, not the provider id.
    let (origin, status, stored_value): (String, String, Option<bool>) = env
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT origin, status, result_bool FROM generated_capability_calls",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|error| error.to_string())
        })
        .unwrap();
    assert_eq!(origin, "conversation");
    assert_eq!(status, "succeeded");
    assert_eq!(stored_value, Some(true));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "operator-only SAAA E2E against the live ContextStill daemon"]
async fn context_still_search_runs_through_the_real_conversation_tool_loop() {
    let (env, mut state, _revision_id, _generated_name) = ready_state().await;
    state.context_still_search =
        crate::memory::context_still_search::ContextStillSearchClient::from_environment();
    assert!(state.context_still_search.is_configured());
    let run_id = "context-still-conversation-e2e";
    let session = seeded_state(&env, &state, run_id);
    let (endpoint, server) = fixture(vec![
        (
            200,
            tool_call_stream(
                crate::memory::context_still_search::SEARCH_KNOWLEDGE_TOOL_NAME,
                r#"{"query":"SAAA cross-cutting diagnostics implementation verification","limit":1}"#,
            ),
            0,
        ),
        (200, content_stream("ContextStillを参照して実装計画を作成しました。"), 0),
    ])
    .await;
    let mut input = turn_input(run_id);
    input.content = "SAAAに横断的な診断機能を追加する実装計画を、再利用できる設計ルールと検証方法を踏まえて作ってください。".into();
    env.writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE conversation_messages SET content=?1 WHERE id='m2a-source'",
                    [&input.content],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("source task updates");
    let mut task_history = history();
    task_history[0].content = input.content.clone();
    let result = run(
        &endpoint,
        None,
        "fixture",
        &task_history,
        10_000,
        ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 1000,
            input: &input,
            on_event: &Sink::default(),
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
    .expect("SAAA conversation tool loop completes");
    assert_eq!(result, "ContextStillを参照して実装計画を作成しました。");

    let requests = server.await.unwrap();
    assert!(requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["function"]["name"]
            == crate::memory::context_still_search::SEARCH_KNOWLEDGE_TOOL_NAME));
    let content = tool_message_content(&requests);
    assert_eq!(content["source"], "context_still", "{content}");
    assert_eq!(content["memoryType"], "knowledge", "{content}");
    assert_eq!(content["trust"]["instructionAuthority"], "none");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c02_a_mixed_batch_with_an_unoffered_name_is_refused_before_any_call() {
    let (env, state, revision_id, name) = ready_state().await;
    let run_id = "m2a-mixed";
    let session = seeded_state(&env, &state, run_id);
    let arguments = json!({ "enabled": true, "suspended": false }).to_string();
    let mixed = format!(
        "{}{}data: [DONE]\r\n\r\n",
        chunk(
            json!({"tool_calls":[
                {"index":0,"id":"gc-call","type":"function",
                 "function":{"name":name,"arguments":arguments}},
                {"index":1,"id":"unknown-call","type":"function",
                 "function":{"name":"definitely_not_a_tool","arguments":"{}"}}
            ]}),
            Value::Null
        ),
        chunk(json!({}), json!("tool_calls"))
    );
    let (endpoint, server) = fixture(vec![(200, mixed, 0)]).await;
    let input = turn_input(run_id);
    let error = run(
        &endpoint,
        None,
        "fixture",
        &history(),
        10_000,
        ModelStreamContext {
            reasoning_effort: "provider-default",
            max_output_tokens: 1000,
            input: &input,
            on_event: &Sink::default(),
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
    .expect_err("a mixed batch is a protocol error");
    assert_eq!(error, Error::failed(Failure::Protocol, false));
    assert_eq!(
        server.await.unwrap().len(),
        1,
        "no retry, no second request"
    );
    let calls: i64 = env
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM generated_capability_calls WHERE revision_id = ?1",
                    rusqlite::params![revision_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .unwrap();
    assert_eq!(calls, 0, "no offered call may run in a rejected batch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn c03_generated_tools_need_persistence_and_respect_the_call_budget() {
    let (_env, state, _revision_id, name) = ready_state().await;
    let input = turn_input("m2a-budget");
    let without_persistence = available_agent_tools(None, &input, 0, 0, 0);
    assert!(without_persistence.generated.is_empty());

    let persistence = Some(crate::ProviderOutputPersistence {
        state: &state,
        session_id: "unused",
        world: None,
    });
    let offered = available_agent_tools(persistence, &input, 0, 0, 0);
    assert_eq!(offered.generated.descriptors().len(), 1);
    assert!(offered.generated.resolve(&name).is_some());

    let over_budget = available_agent_tools(persistence, &input, 12, 0, 0);
    assert!(
        over_budget.generated.is_empty(),
        "the 12-call admission rule also bounds generated tools"
    );
}
