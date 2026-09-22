#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ipc_contract::RuntimeEvent,
        persistence::{SqliteReaders, SqliteWriter},
        RunCancellation, StartTurnInput,
    };
    use rusqlite::Connection;
    use std::{path::PathBuf, sync::Arc, time::Duration};

    #[test]
    fn search_inputs_are_bounded_and_do_not_accept_model_supplied_paths() {
        assert!(parse_arguments(
            SEARCH_KNOWLEDGE_TOOL_NAME,
            r#"{"query":"release safety","types":["rule"],"limit":3}"#
        )
        .is_ok());
        assert_eq!(
            parse_arguments(
                SEARCH_EPISODES_TOOL_NAME,
                r#"{"query":"release safety","repoPath":"/tmp/other"}"#
            ),
            Err(SearchError::InvalidInput)
        );
        assert_eq!(
            parse_arguments(SEARCH_EPISODES_TOOL_NAME, r#"{"query":"","limit":10}"#),
            Err(SearchError::InvalidInput)
        );
    }

    #[test]
    fn results_are_compacted_and_marked_untrusted() {
        let remote = json!({
            "content": [{
                "type": "text",
                "text": json!({
                    "diagnostics": {"private": "discard"},
                    "items": [{
                        "id": "discard",
                        "title": "Past release",
                        "situation": "A migration was required",
                        "outcome": "Succeeded",
                        "lesson": "Verify the rollback path",
                        "outcomeKind": "success",
                        "score": 91,
                        "metadata": {"private": "discard"}
                    }]
                }).to_string()
            }]
        });
        let compact = compact_result(SEARCH_EPISODES_TOOL_NAME, &remote).expect("result compacts");
        let value: Value = serde_json::from_str(&compact).expect("compact result is json");
        assert_eq!(value["trust"]["instructionAuthority"], "none");
        assert_eq!(value["memoryType"], "episode");
        assert!(value["items"][0].get("metadata").is_none());
        assert!(value["items"][0].get("id").is_none());
    }

    #[tokio::test]
    #[ignore = "operator-only live ContextStill search canary"]
    async fn live_context_still_searches_episode_cards() {
        let client = ContextStillSearchClient::from_environment();
        assert!(client.is_configured());
        let result = client
            .search(
                SEARCH_EPISODES_TOOL_NAME,
                r#"{"query":"software implementation verification","limit":1}"#,
                Some(env!("CARGO_MANIFEST_DIR")),
            )
            .await
            .expect("live search succeeds");
        let value: Value = serde_json::from_str(&result).expect("result is json");
        assert_eq!(value["source"], "context_still");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "operator-only live SAAA conversation using an isolated app database"]
    async fn live_saaa_conversation_proactively_searches_context_still() {
        let database = PathBuf::from(
            env::var("SAAA_CONTEXT_STILL_E2E_DATABASE")
                .expect("explicit isolated SAAA app database"),
        );
        assert!(database.is_absolute() && database.is_file());

        let dummy = Connection::open_in_memory().expect("dummy database opens");
        crate::initialize_database(&dummy).expect("dummy database initializes");
        let mut state = crate::test_support::app_state(dummy);
        state.sqlite_writer =
            Arc::new(SqliteWriter::open(&database).expect("isolated app database ownership"));
        state.sqlite_readers = SqliteReaders::open(&database).expect("isolated readers open");
        state.data_directory = database
            .parent()
            .expect("database has a parent")
            .to_path_buf();
        state.context_still_search = ContextStillSearchClient::from_environment();
        assert!(state.context_still_search.is_configured());
        SEARCH_CALL_LOG
            .lock()
            .expect("ContextStill test call log locks")
            .clear();

        let run_id = crate::new_id("context_still_e2e");
        let input = StartTurnInput {
            run_id: run_id.clone(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            content: "SAAAの会話ランタイムに新しい横断的な診断機能を追加すると仮定し、過去の実装上の教訓と再利用できる設計ルールを踏まえて、変更箇所・リスク・検証方法を含む実装計画を作ってください。単なる一般論ではなく、このリポジトリで実行可能な計画にしてください。".into(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        };
        let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
        let cancellation = Arc::new(RunCancellation::default());
        let outcome = tokio::time::timeout(
            Duration::from_secs(180),
            crate::runtime::turns::execute_turn(
                &state,
                &input,
                &events,
                cancellation.clone(),
                None,
            ),
        )
        .await;
        if outcome.is_err() {
            cancellation.cancel();
        }
        outcome
            .expect("SAAA conversation deadline")
            .unwrap_or_else(|error| panic!("SAAA conversation failed: {}", error.message));

        let calls = SEARCH_CALL_LOG
            .lock()
            .expect("ContextStill test call log locks")
            .clone();
        println!("SAAA ContextStill calls for {run_id}: {calls:?}");
        assert!(
            calls.iter().any(|name| is_search_tool(name)),
            "the SAAA conversation completed without starting ContextStill exploration"
        );
    }
}
