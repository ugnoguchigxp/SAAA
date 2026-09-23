use super::*;
fn input(run: &str, message: &str, project: &str) -> StartTurnInput {
    StartTurnInput {
        run_id: run.into(),
        conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
        content: message.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: vec![crate::TurnScopeRef {
            kind: "project".into(),
            id: project.into(),
            relation: "focus".into(),
        }],
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    }
}

fn database() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&connection).unwrap();
    for project in ["project_a", "project_b"] {
        connection
            .execute(
                "INSERT INTO context_scopes VALUES(?1,'project',?2,'active','1')",
                params![format!("project:{project}"), project],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO context_scope_epochs VALUES(?1,0)",
                [format!("project:{project}")],
            )
            .unwrap();
    }
    connection
}

fn prepare(connection: &Connection, input: &StartTurnInput, message_id: &str) -> ScopeSnapshot {
    connection
        .execute(
            "INSERT INTO conversation_messages VALUES(?1,?2,'user',?3,'1')",
            params![message_id, crate::PRIMARY_CONVERSATION_ID, input.content],
        )
        .unwrap();
    connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES(?1,?2,'conversation.respond','running',?3,'1')",params![input.run_id,crate::PRIMARY_CONVERSATION_ID,message_id]).unwrap();
    resolve(connection, input, message_id, true).unwrap()
}

#[test]
fn explicit_project_history_never_crosses_focus() {
    let connection = database();
    let a = input("run_a", "current A", "project_a");
    let b = input("run_b", "private B", "project_b");
    let _ = prepare(&connection, &b, "message_b");
    connection
        .execute(
            "UPDATE runtime_runs SET status='completed' WHERE id='run_b'",
            [],
        )
        .unwrap();
    let a_scope = prepare(&connection, &a, "message_a");
    let loaded = crate::memory::context_window::load(
        &connection,
        crate::PRIMARY_CONVERSATION_ID,
        "message_a",
        &a_scope,
    )
    .unwrap();
    let projected = crate::memory::context_window::compose(loaded).unwrap();
    let text = projected
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("current A"));
    assert!(!text.contains("private B"));
}

#[test]
fn missing_scope_is_persisted_without_guessing() {
    let connection = database();
    let missing = input("run_missing", "question", "not_registered");
    let snapshot = prepare(&connection, &missing, "message_missing");
    assert_eq!(snapshot.status, "missing");
    assert_eq!(
        snapshot.reason_code.as_deref(),
        Some("scope-not-registered")
    );
}

#[test]
fn unrelated_scope_change_does_not_invalidate_generation() {
    let connection = database();
    let a = input("run_a", "current A", "project_a");
    let _ = prepare(&connection, &a, "message_a");
    let state = crate::test_support::app_state(connection);
    let generation = crate::runtime::context::generation::begin_direct_dispatched(
        &state,
        "run_a",
        "provider",
        "reasoning",
        b"request",
    )
    .unwrap();
    state
        .sqlite_writer
        .write(|connection| {
            let b = input("run_b", "current B", "project_b");
            let _ = prepare(connection, &b, "message_b");
            Ok(())
        })
        .unwrap();
    generation.complete().unwrap();

    let stale = crate::runtime::context::generation::begin_direct_dispatched(
        &state,
        "run_a",
        "provider",
        "tool-followup",
        b"request-2",
    )
    .unwrap();
    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE conversation_messages SET content='edited A' WHERE id='message_a'",
                    [],
                )
                .map_err(database_error)?;
            Ok(())
        })
        .unwrap();
    assert!(stale.complete().is_err());
}
