use crate::runtime::event_hub::RuntimeEventSender;
use std::sync::{Arc, Mutex};

struct DeltaSink(Arc<Mutex<Vec<String>>>);
impl RuntimeEventSender for DeltaSink {
    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(DeltaSink(self.0.clone()))
    }
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        if let RuntimeEvent::Delta { text, .. } = event {
            self.0.lock().expect("delta lock").push(text);
        }
        Ok(())
    }
}
#[tokio::test]
async fn acceptance_core_text_turn_persists_and_is_recalled() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database_path = directory.path().join("saaa.sqlite3");
    let request = "朝の予定をメモして";

    let deltas = Arc::new(Mutex::new(Vec::new()));
    {
        let (endpoint, _captures, server) = spawn_llm_http_fixture(vec![
            LlmHttpStep::Delta("了解"),
            LlmHttpStep::Delta("しました"),
            LlmHttpStep::Complete,
        ])
        .await;
        let connection = Connection::open(&database_path).expect("database opens");
        initialize_database(&connection).expect("database initializes");
        let state = app_state(connection);
        let input = StartTurnInput {
            run_id: "run_acceptance-core".to_string(),
            conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
            content: request.to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        crate::test_support::prepare_user_turn(&state, &input).expect("runtime prepares");
        let _session_id = begin_test_provider_session(
            &state,
            &input.run_id,
            "acceptance-fixture",
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
            ..direct_provider("acceptance-fixture", "local")
        };
        let sink = DeltaSink(deltas.clone());
        let outcome = crate::providers::chat_completions::run_with_options(
            &provider.endpoint,
            None,
            &provider.model,
            &history,
            5_000,
            ModelStreamContext {
                reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
                max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
                input: &input,
                on_event: &sink,
                cancellation: Arc::new(RunCancellation::default()),
                context_health: "green",
                context_sources: &[],
                context_omissions: &[],
                output_persistence: None,
            },
            crate::providers::chat_completions::RequestMode::Stream,
            &saaa_larm_session::http_api::LlmOptions { tools: false, ..Default::default() },
        )
        .await;
        server.await.expect("fixture server joins");
        let content = outcome.expect("bounded diagnostic stream completes");
        assert_eq!(content, "了解しました");
        crate::providers::session_store::persist_conversation_success_with_state(
            &state,
            &input,
            &content,
            |_connection, _message| Ok(()),
        )
        .expect("assistant message persists");
        let connection = state.sqlite_writer.lock().expect("database lock");
        let saved: Vec<String> = connection
            .prepare(
                "SELECT role || ':' || content FROM conversation_messages WHERE conversation_id=?1 ORDER BY created_at, id",
            )
            .expect("message query")
            .query_map([&input.conversation_id], |row| row.get(0))
            .expect("message rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("messages");
        assert!(
            saved.iter().any(|row| row == &format!("user:{request}")),
            "saved messages: {saved:?}"
        );
        assert!(
            saved.iter().any(|row| row.contains("assistant:了解しました")),
            "saved messages: {saved:?}"
        );
    }
    assert!(
        deltas.lock().expect("delta lock").len() >= 2,
        "expected at least two streamed deltas"
    );

    let connection = Connection::open(&database_path).expect("database reopens");
    let state = app_state(connection);
    let input = StartTurnInput {
        run_id: "run_acceptance-core-recall".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "さっきの予定を思い出して".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    crate::test_support::prepare_user_turn(&state, &input).expect("recall turn prepares");
    let recalled = crate::providers::stream::execute_recall_tool(
        Some(ProviderOutputPersistence {
            state: &state,
            session_id: "acceptance-reopen",
            world: None,
        }),
        &input,
        &crate::runtime::agent_tools::AgentToolCall {
            id: "acceptance-recall".to_string(),
            name: "recall_conversation".to_string(),
            arguments: r#"{"query":"朝の予定"}"#.to_string(),
        },
    );
    assert!(
        recalled.contains(request),
        "recall after reopen should include the original request, got {recalled}"
    );
}
