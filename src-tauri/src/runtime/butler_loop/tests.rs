use super::*;
use rusqlite::{params, Connection};

fn db() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE conversations (
               id TEXT PRIMARY KEY,
               title TEXT,
               task_mode TEXT NOT NULL,
               created_at TEXT NOT NULL,
               updated_at TEXT NOT NULL
             );
             CREATE TABLE conversation_messages (
               id TEXT PRIMARY KEY,
               conversation_id TEXT NOT NULL,
               role TEXT NOT NULL,
               content TEXT NOT NULL,
               created_at TEXT NOT NULL
             );
             CREATE TABLE runtime_runs (
               id TEXT PRIMARY KEY,
               conversation_id TEXT NOT NULL,
               route_kind TEXT NOT NULL,
               status TEXT NOT NULL
             );
             INSERT INTO conversations VALUES('c','t','conversation','1','1');",
        )
        .unwrap();
    ensure_schema(&connection).unwrap();
    connection
}

#[test]
fn direct_answer_does_not_invent_a_preface() {
    let actions = from_provider_turn(ProviderTurn {
        content: Some("42".into()),
        tool_calls: vec![],
        finish: ProviderFinish::Stop,
    });
    assert_eq!(actions, vec![AgentAction::Answer { text: "42".into() }]);
}

#[test]
fn tool_without_text_has_no_fabricated_speech() {
    let actions = from_provider_turn(ProviderTurn {
        content: Some("  ".into()),
        tool_calls: vec![ToolCall {
            id: "t1".into(),
            name: "search".into(),
            arguments_json: "{}".into(),
        }],
        finish: ProviderFinish::ToolCalls,
    });
    assert!(matches!(
        &actions[..],
        [AgentAction::UseTool { preface: None, .. }]
    ));
}

#[test]
fn preface_and_tool_stay_one_action() {
    let actions = from_provider_turn(ProviderTurn {
        content: Some("調べます".into()),
        tool_calls: vec![ToolCall {
            id: "t1".into(),
            name: "search".into(),
            arguments_json: "{}".into(),
        }],
        finish: ProviderFinish::ToolCalls,
    });
    assert!(matches!(
        &actions[..],
        [AgentAction::UseTool {
            preface: Some(text),
            ..
        }] if text == "調べます"
    ));
}

#[test]
fn message_and_event_commit_together_and_roll_back() {
    let mut connection = db();
    {
        let tx = connection.transaction().unwrap();
        commit_visible_message(
            &tx,
            "c",
            "m1",
            "assistant",
            "先に伝えます",
            "2",
            Some("run-1"),
            "message_committed",
        )
        .unwrap();
        tx.rollback().unwrap();
    }
    let messages: i64 = connection
        .query_row("SELECT COUNT(*) FROM conversation_messages", [], |row| {
            row.get(0)
        })
        .unwrap();
    let events: i64 = connection
        .query_row("SELECT COUNT(*) FROM conversation_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!((messages, events), (0, 0));
}

#[test]
fn sequence_is_monotonic_and_duplicate_seq_fails() {
    let mut connection = db();
    let tx = connection.transaction().unwrap();
    let first = commit_visible_message(
        &tx,
        "c",
        "m1",
        "assistant",
        "a",
        "2",
        Some("run-1"),
        "message_committed",
    )
    .unwrap();
    let second = commit_visible_message(
        &tx,
        "c",
        "m2",
        "assistant",
        "b",
        "3",
        Some("run-1"),
        "message_committed",
    )
    .unwrap();
    assert!(second > first);
    let duplicate = tx.execute(
        "INSERT INTO conversation_events(conversation_id, seq, kind, created_at) VALUES('c', ?1, 'x', '4')",
        params![first],
    );
    assert!(duplicate.is_err());
    tx.commit().unwrap();
    let replay: Vec<String> = {
        let mut statement = connection
            .prepare(
                "SELECT message_id FROM conversation_events WHERE conversation_id='c' AND seq > 0 ORDER BY seq",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(replay, vec!["m1".to_string(), "m2".to_string()]);
}

#[test]
fn pending_inputs_are_consumed_once_in_order() {
    let mut connection = db();
    let tx = connection.transaction().unwrap();
    commit_visible_message(&tx, "c", "u1", "user", "変更", "2", None, "user_message").unwrap();
    commit_visible_message(&tx, "c", "u2", "user", "中止", "3", None, "user_message").unwrap();
    accept_run_input(&tx, "c", "run-1", "u1").unwrap();
    accept_run_input(&tx, "c", "run-1", "u2").unwrap();
    let first = ledger::consume_pending_inputs(&tx, "c", "run-1").unwrap();
    let second = ledger::consume_pending_inputs(&tx, "c", "run-1").unwrap();
    assert_eq!(first, vec!["u1".to_string(), "u2".to_string()]);
    assert!(second.is_empty());
    tx.commit().unwrap();
}

#[test]
fn additional_input_attaches_only_to_the_selected_active_conversation_run() {
    let mut connection = db();
    connection
        .execute_batch(
            "INSERT INTO runtime_runs VALUES('selected','c','conversation.respond','running');
         INSERT INTO runtime_runs VALUES('coding','c','coding.assist','running');
         INSERT INTO runtime_runs VALUES('finished','c','conversation.respond','completed');
         INSERT INTO runtime_runs VALUES('newer','c','conversation.respond','completed');",
        )
        .unwrap();
    let tx = connection.transaction().unwrap();
    commit_visible_message(&tx, "c", "u1", "user", "変更", "2", None, "user_message").unwrap();
    assert!(ledger::attach_to_running(&tx, "c", "coding", "u1").is_err());
    assert!(ledger::attach_to_running(&tx, "c", "finished", "u1").is_err());
    assert!(!ledger::attach_to_running(&tx, "c", "selected", "u1").unwrap());
    let attached: Vec<String> = tx
        .prepare("SELECT run_id FROM conversation_run_inputs")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(attached, vec!["selected"]);
}

#[test]
fn consuming_a_batch_rolls_back_if_any_input_is_invalid() {
    let mut connection = db();
    let tx = connection.transaction().unwrap();
    commit_visible_message(&tx, "c", "u1", "user", "変更", "2", None, "user_message").unwrap();
    accept_run_input(&tx, "c", "run-1", "u1").unwrap();
    tx.commit().unwrap();

    let result = mark_inputs_consumed(
        &mut connection,
        "c",
        "run-1",
        &["u1".into(), "missing".into()],
    );
    assert!(result.is_err());
    let state: String = connection
        .query_row(
            "SELECT state FROM conversation_run_inputs WHERE message_id='u1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "pending");
}

#[test]
fn input_after_final_gate_is_preserved_for_the_next_run() {
    let mut connection = db();
    connection
        .execute(
            "INSERT INTO runtime_runs VALUES('selected','c','conversation.respond','running')",
            [],
        )
        .unwrap();
    let tx = connection.transaction().unwrap();
    tx.execute(
        "INSERT INTO conversation_run_input_gate(run_id,closed) VALUES('selected',1)",
        [],
    )
    .unwrap();
    commit_visible_message(&tx, "c", "u1", "user", "続けて", "2", None, "user_message").unwrap();
    ledger::attach_to_running(&tx, "c", "selected", "u1").unwrap();
    tx.commit().unwrap();
    assert!(peek_pending_user_texts(&connection, "c", "selected")
        .unwrap()
        .is_empty());
    connection
        .execute(
            "UPDATE runtime_runs SET status='completed' WHERE id='selected'",
            [],
        )
        .unwrap();
    assert_eq!(
        ledger::first_unconsumed_terminal_input(&connection, "c").unwrap(),
        Some(("u1".into(), "続けて".into(), "completed".into()))
    );
    connection
        .execute(
            "UPDATE conversation_run_inputs SET state='consumed' WHERE message_id='u1'",
            [],
        )
        .unwrap();
    assert!(ledger::first_unconsumed_terminal_input(&connection, "c")
        .unwrap()
        .is_none());
}

#[test]
fn completed_run_follow_up_starts_once_without_duplicating_the_user_message() {
    let connection = Connection::open_in_memory().unwrap();
    crate::initialize_database(&connection).unwrap();
    let state = crate::test_support::app_state(connection);
    let input = |run_id: &str, content: &str, retry_input_message_id: Option<String>| {
        crate::StartTurnInput {
            run_id: run_id.into(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            content: content.into(),
            workspace_path: None,
            retry_input_message_id,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        }
    };
    crate::runtime::turns::prepare_runtime_run(&state, &input("run-first", "最初", None)).unwrap();
    state
        .sqlite_writer
        .write(|connection| {
            let tx = connection.transaction().map_err(crate::database_error)?;
            commit_visible_message(
                &tx,
                crate::PRIMARY_CONVERSATION_ID,
                "follow-up",
                "user",
                "追加",
                "2",
                None,
                "user_message",
            )
            .map_err(crate::database_error)?;
            ledger::attach_to_running(
                &tx,
                crate::PRIMARY_CONVERSATION_ID,
                "run-first",
                "follow-up",
            )
            .map_err(crate::database_error)?;
            tx.execute(
                "UPDATE runtime_runs SET status='completed' WHERE id='run-first'",
                [],
            )
            .map_err(crate::database_error)?;
            tx.commit().map_err(crate::database_error)?;
            Ok(())
        })
        .unwrap();
    let next = input("run-second", "追加", Some("follow-up".into()));
    crate::runtime::turns::prepare_runtime_run(&state, &next).unwrap();
    assert!(crate::runtime::turns::prepare_runtime_run(
        &state,
        &input("run-third", "追加", Some("follow-up".into()))
    )
    .is_err());
    let (messages, claimed): (i64, String) =
        state
            .sqlite_readers
            .read(|connection| {
                Ok((
            connection.query_row(
                "SELECT COUNT(*) FROM conversation_messages WHERE id='follow-up'",
                [],
                |row| row.get(0),
            ).map_err(crate::database_error)?,
            connection.query_row(
                "SELECT state FROM conversation_run_inputs WHERE message_id='follow-up'",
                [],
                |row| row.get(0),
            ).map_err(crate::database_error)?,
        ))
            })
            .unwrap();
    assert_eq!(messages, 1);
    assert_eq!(claimed, "consumed");
    let work_links: Vec<String> = state.sqlite_readers.read(|connection| {
        connection
            .prepare("SELECT work_id FROM conversation_work_runs WHERE run_id IN ('run-first','run-second') ORDER BY run_id")
            .map_err(crate::database_error)?
            .query_map([], |row| row.get(0))
            .map_err(crate::database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(crate::database_error)
    }).unwrap();
    assert_eq!(work_links.len(), 2);
    assert_eq!(work_links[0], work_links[1]);
}

#[test]
fn initial_and_final_messages_share_the_ordered_conversation_ledger() {
    let connection = Connection::open_in_memory().unwrap();
    crate::initialize_database(&connection).unwrap();
    let state = crate::test_support::app_state(connection);
    let input = crate::StartTurnInput {
        run_id: "ledger-run".into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: "質問".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    crate::runtime::turns::prepare_runtime_run(&state, &input).unwrap();
    let answer = crate::providers::session_store::persist_conversation_success_with_state(
        &state,
        &input,
        "回答",
        |_, _| Ok(()),
    )
    .unwrap();
    let events: Vec<(String, String)> = state
        .sqlite_readers
        .read(|connection| {
            connection
                .prepare(
                    "SELECT kind,message_id FROM conversation_events
                 WHERE conversation_id=?1 ORDER BY seq",
                )
                .map_err(crate::database_error)?
                .query_map([crate::PRIMARY_CONVERSATION_ID], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(crate::database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].0, "user_message");
    assert_eq!(events[1], ("message_completed".into(), answer.id));
    let work_status: String = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT status FROM conversation_work_state WHERE run_id='ledger-run'",
                    [],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .unwrap();
    assert_eq!(work_status, "completed");
}

#[test]
fn presentation_receipt_is_bound_to_the_committed_message() {
    let mut connection = db();
    let message = commit_preface(&mut connection, "c", "run-1", "調べます")
        .unwrap()
        .unwrap();
    assert!(!message_was_presented(&connection, &message.id).unwrap());
    assert!(
        !ledger::acknowledge_message_presented(&connection, "c", "wrong", &message.id).unwrap()
    );
    assert!(ledger::acknowledge_message_presented(&connection, "c", "run-1", &message.id).unwrap());
    assert!(message_was_presented(&connection, &message.id).unwrap());
    ledger::mark_presentation_unconfirmed(&connection, &message.id).unwrap();
    assert!(message_was_presented(&connection, &message.id).unwrap());
}

#[test]
fn stale_work_revision_does_not_overwrite() {
    let mut connection = db();
    let tx = connection.transaction().unwrap();
    let initial = WorkState {
        work_id: "w".into(),
        conversation_id: "c".into(),
        run_id: Some("run-1".into()),
        revision: 1,
        purpose: "探す".into(),
        confirmed_refs_json: "[]".into(),
        open_questions_json: "[]".into(),
        next_options_json: "[]".into(),
        status: "running".into(),
        updated_at: "2".into(),
    };
    assert!(update_work_state(&tx, &initial, None).unwrap());
    let mut raced = initial.clone();
    raced.revision = 2;
    raced.purpose = "新しい目的".into();
    assert!(!update_work_state(&tx, &raced, Some(0)).unwrap());
    raced.revision = 2;
    assert!(update_work_state(&tx, &raced, Some(1)).unwrap());
    tx.commit().unwrap();
    let stored = load_work_state(&connection, "w").unwrap().unwrap();
    assert_eq!(stored.purpose, "新しい目的");
    assert_eq!(stored.revision, 2);
}
