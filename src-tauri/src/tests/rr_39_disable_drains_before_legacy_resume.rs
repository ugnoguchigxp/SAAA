use super::*;
#[test]
pub(super) fn rr_39_disable_drains_before_legacy_resume() {
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let mut documents = default_settings_input();
    documents
        .iter_mut()
        .find(|document| document.namespace == "providers.model")
        .expect("provider settings")
        .value_json = json!({
        "harness": { "address": "http://localhost:9810" },
        "providers": [{
            "kind":"openai-compatible","id":"author-provider","enabled":true,
            "label":"Author","location":"local","endpoint":"http://127.0.0.1:9/v1",
            "model":"author-model","authentication":"none"
        }],
        "reasoningEffort":"medium"
    });
    documents
        .iter_mut()
        .find(|document| document.namespace == "routing.roles")
        .expect("role policy")
        .value_json = single_role_policy("author-provider");
    let task_routes = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.tasks")
        .expect("task routes");
    task_routes.value_json["voiceSpeak"]["source"] = json!("harness");
    task_routes.value_json["voiceSpeak"]["providerId"] = Value::Null;
    save_settings_documents_to_connection(
        &mut state.sqlite_writer.lock().expect("database lock"),
        &documents,
    )
    .expect("settings save");

    let input = |run_id: &str| StartTurnInput {
        run_id: run_id.into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: format!("request for {run_id}"),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    prepare_runtime_run(&state, &input("rr-disable-active")).expect("routing receipt");
    let cancellation = Arc::new(RunCancellation::default());
    register_active_run(&state, "rr-disable-active", cancellation.clone())
        .expect("active run registers");
    state
        .sqlite_writer
        .write(|connection| {
            let (step_id, revision): (String, u32) = connection
                .query_row(
                    "SELECT id,revision FROM rr_steps WHERE root_id='rr-disable-active' AND status='running'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|error| error.to_string())?;
            let link = crate::role_routing::tool_ledger::ToolLink {
                id: "rr-disable-link".into(),
                root_id: "rr-disable-active".into(),
                step_id,
                revision,
                operation_key: "rr-disable-operation".into(),
                invocation_id: None,
                dispatch_state: "reserved".into(),
                result_ref: None,
            };
            crate::role_routing::tool_ledger::reserve(connection, &link, 1)?;
            crate::role_routing::tool_ledger::settle(
                connection,
                "rr-disable-active",
                "rr-disable-operation",
                Some("rr-disable-invocation"),
                None,
                "dispatched",
                2,
            )?;
            Ok(())
        })
        .expect("in-flight side effect");

    documents
        .iter_mut()
        .find(|document| document.namespace == "routing.roles")
        .expect("role policy")
        .value_json = json!({"schemaVersion": 1, "enabled": false});
    crate::persistence::app_commands::save_settings_documents(
        &state,
        SaveSettingsDocumentsInput { documents },
    )
    .expect("disable routing");
    assert!(
        cancellation.is_cancelled(),
        "the active child must be cancelled"
    );
    let (phase, step_status, link_status) = state
        .sqlite_writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT r.phase,s.status,l.dispatch_state
                     FROM rr_roots r JOIN rr_steps s ON s.root_id=r.root_id
                     JOIN rr_tool_links l ON l.root_id=r.root_id
                     WHERE r.root_id='rr-disable-active'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(|error| error.to_string())
        })
        .expect("cancelled ledger");
    assert_eq!(
        (phase.as_str(), step_status.as_str(), link_status.as_str()),
        ("cancelled", "cancelled", "dispatched")
    );

    let messages_before = state
        .sqlite_writer
        .read_serialized(|connection| {
            connection
                .query_row("SELECT count(*) FROM conversation_messages", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|error| error.to_string())
        })
        .expect("message count");
    assert!(prepare_runtime_run(&state, &input("rr-disable-blocked"))
        .expect_err("legacy must wait for the old side effect")
        .contains("still draining"));
    let messages_after = state
        .sqlite_writer
        .read_serialized(|connection| {
            connection
                .query_row("SELECT count(*) FROM conversation_messages", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|error| error.to_string())
        })
        .expect("message count");
    assert_eq!(
        messages_after, messages_before,
        "a drain rejection must not save input"
    );

    state
        .sqlite_writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE runtime_runs SET status='cancelled' WHERE id='rr-disable-active'",
                    [],
                )
                .map_err(|error| error.to_string())?;
            crate::role_routing::tool_ledger::settle(
                connection,
                "rr-disable-active",
                "rr-disable-operation",
                Some("rr-disable-invocation"),
                Some("rr-disable-result"),
                "settled",
                3,
            )?;
            Ok(())
        })
        .expect("side effect drains");
    prepare_runtime_run(&state, &input("rr-disable-legacy")).expect("legacy resumes");
    assert_eq!(
        state
            .sqlite_writer
            .read_serialized(|connection| connection
                .query_row(
                    "SELECT count(*) FROM rr_roots WHERE root_id='rr-disable-legacy'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .map_err(|error| error.to_string()))
            .expect("legacy root count"),
        0,
        "disabled routing must not create a new routing root"
    );
}
#[tokio::test]
pub(super) async fn rr_05_normal_turn_two_steps_commits_only_the_final_answer() {
    let (front_endpoint, front_requests, front_server) =
        spawn_llm_http_fixture(vec![LlmHttpStep::Delta("ack draft"), LlmHttpStep::Complete]).await;
    let (reason_endpoint, reason_requests, reason_server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta("reasoner final"),
        LlmHttpStep::Complete,
    ])
    .await;
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let mut documents = default_settings_input();
    documents
        .iter_mut()
        .find(|document| document.namespace == "providers.model")
        .expect("provider settings")
        .value_json = json!({
    "harness": { "address": "http://localhost:9810" },
    "providers": [{
        "kind": "openai-compatible", "id": "front-provider", "enabled": true,
        "label": "Front", "location": "local", "endpoint": front_endpoint,
        "model": "front-model", "authentication": "none"
    }, {
        "kind": "openai-compatible", "id": "reason-provider", "enabled": true,
        "label": "Reason", "location": "local", "endpoint": reason_endpoint,
        "model": "reason-model", "authentication": "none"
    }],
        "reasoningEffort": "medium"
    });
    let task_routes = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.tasks")
        .expect("task routes");
    task_routes.value_json["voiceSpeak"]["source"] = json!("harness");
    task_routes.value_json["voiceSpeak"]["providerId"] = Value::Null;
    let role_policy = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.roles")
        .expect("role policy");
    role_policy.value_json = two_step_role_policy("front-provider", "reason-provider");
    save_settings_documents_to_connection(
        &mut state.sqlite_writer.lock().expect("database lock"),
        &documents,
    )
    .expect("settings save");
    let input = StartTurnInput {
        run_id: "rr-two-step-run".into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: "two step request".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_events = events.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            captured_events.lock().expect("events").push(value);
        }
        Ok(())
    });
    execute_turn(
        &state,
        &input,
        &channel,
        Arc::new(RunCancellation::default()),
        None,
    )
    .await
    .expect("two-step turn completes");
    front_server.await.expect("front server");
    reason_server.await.expect("reason server");

    assert_eq!(front_requests.lock().expect("front requests").len(), 1);
    assert_eq!(reason_requests.lock().expect("reason requests").len(), 1);
    let database = state.sqlite_writer.lock().expect("database lock");
    let assistant_messages: Vec<String> = database
        .prepare("SELECT content FROM conversation_messages WHERE conversation_id=?1 AND role='assistant' ORDER BY created_at,id")
        .expect("messages statement")
        .query_map([PRIMARY_CONVERSATION_ID], |row| row.get(0))
        .expect("messages")
        .collect::<Result<Vec<_>, _>>()
        .expect("assistant messages");
    assert_eq!(assistant_messages, vec!["reasoner final"]);
    let step_states = database
        .prepare("SELECT status FROM rr_steps WHERE root_id=?1 ORDER BY ordinal")
        .expect("steps statement")
        .query_map([&input.run_id], |row| row.get::<_, String>(0))
        .expect("steps")
        .collect::<Result<Vec<_>, _>>()
        .expect("step states");
    assert_eq!(step_states, vec!["succeeded", "succeeded"]);
    assert_eq!(
        database
            .query_row(
                "SELECT count(*) FROM rr_outputs o JOIN rr_steps s ON s.id=o.step_id WHERE s.root_id=?1 AND o.accepted=1",
                [&input.run_id],
                |row| row.get::<_, i64>(0),
            )
            .expect("accepted outputs"),
        1
    );
    drop(database);
    let event_log = events.lock().expect("events").join("\n");
    assert_eq!(
        event_log.matches("\"type\":\"messageCompleted\"").count(),
        1
    );
    assert!(!event_log.contains("ack draft"));
    assert_eq!(event_log.matches("\"type\":\"delta\"").count(), 0);
}
#[tokio::test]
pub(super) async fn rr_09_partial_role_step_leaks_no_draft_and_cancels_remaining_work() {
    use std::io::ErrorKind;
    use std::net::TcpListener;

    let (front_endpoint, _, front_server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta("private partial draft"),
        LlmHttpStep::Disconnect,
    ])
    .await;
    let reason_listener = TcpListener::bind("127.0.0.1:0").expect("reason listener");
    let reason_address = reason_listener.local_addr().expect("reason address");
    reason_listener
        .set_nonblocking(true)
        .expect("reason nonblocking");
    let reason_hit = Arc::new(AtomicBool::new(false));
    let reason_hit_for_server = reason_hit.clone();
    let reason_server = thread::spawn(move || {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            match reason_listener.accept() {
                Ok(_) => {
                    reason_hit_for_server.store(true, Ordering::SeqCst);
                    return;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let mut documents = default_settings_input();
    documents
        .iter_mut()
        .find(|document| document.namespace == "providers.model")
        .expect("provider settings")
        .value_json = json!({
        "harness":{"address":"http://localhost:9810"},
        "providers":[{
            "kind":"openai-compatible","id":"partial-front","enabled":true,
            "label":"Partial front","location":"local","endpoint":front_endpoint,
            "model":"front-model","authentication":"none"
        }, {
            "kind":"openai-compatible","id":"forbidden-reason","enabled":true,
            "label":"Forbidden reason","location":"local",
            "endpoint":format!("http://{reason_address}/v1"),
            "model":"reason-model","authentication":"none"
        }], "reasoningEffort":"medium"
    });
    let task_routes = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.tasks")
        .expect("task routes");
    task_routes.value_json["voiceSpeak"]["source"] = json!("harness");
    task_routes.value_json["voiceSpeak"]["providerId"] = Value::Null;
    documents
        .iter_mut()
        .find(|document| document.namespace == "routing.roles")
        .expect("role policy")
        .value_json = two_step_role_policy("partial-front", "forbidden-reason");
    save_settings_documents_to_connection(
        &mut state.sqlite_writer.lock().expect("database lock"),
        &documents,
    )
    .expect("settings save");
    let input = StartTurnInput {
        run_id: "rr-partial-step-run".into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: "partial two step request".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_events = events.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            captured_events.lock().expect("events").push(value);
        }
        Ok(())
    });
    execute_turn(
        &state,
        &input,
        &channel,
        Arc::new(RunCancellation::default()),
        None,
    )
    .await
    .expect_err("partial role step fails the turn");
    front_server.await.expect("front server");
    reason_server.join().expect("reason server");
    assert!(!reason_hit.load(Ordering::SeqCst));

    let database = state.sqlite_writer.lock().expect("database lock");
    let root_phase: String = database
        .query_row(
            "SELECT phase FROM rr_roots WHERE root_id=?1",
            [&input.run_id],
            |row| row.get(0),
        )
        .expect("root phase");
    let step_states = database
        .prepare("SELECT status FROM rr_steps WHERE root_id=?1 ORDER BY ordinal")
        .expect("steps statement")
        .query_map([&input.run_id], |row| row.get::<_, String>(0))
        .expect("steps")
        .collect::<Result<Vec<_>, _>>()
        .expect("step states");
    let assistant_count: i64 = database
        .query_row(
            "SELECT count(*) FROM conversation_messages WHERE conversation_id=?1 AND role='assistant'",
            [PRIMARY_CONVERSATION_ID],
            |row| row.get(0),
        )
        .expect("assistant count");
    assert_eq!(root_phase, "failed");
    assert_eq!(step_states, vec!["failed", "interrupted"]);
    assert_eq!(assistant_count, 0);
    drop(database);
    let event_log = events.lock().expect("events").join("\n");
    assert!(!event_log.contains("private partial draft"));
    assert_eq!(event_log.matches("\"type\":\"delta\"").count(), 0);
    assert_eq!(
        event_log.matches("\"type\":\"messageCompleted\"").count(),
        0
    );
}
