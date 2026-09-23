use super::*;
#[tokio::test]
pub(super) async fn conversation_route_falls_back_and_persists_completed_message() {
    let (primary_endpoint, _, primary_server) =
        spawn_llm_http_fixture(vec![LlmHttpStep::Fail]).await;
    let (fallback_endpoint, _, fallback_server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta("fallback ok"),
        LlmHttpStep::Complete,
    ])
    .await;
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let conversation = Conversation {
        id: PRIMARY_CONVERSATION_ID.to_string(),
        title: Some(PRIMARY_CONVERSATION_TITLE.to_string()),
        task_mode: "conversation".to_string(),
        created_at: "now".to_string(),
        updated_at: "now".to_string(),
    };
    let mut documents = default_settings_input();
    documents
        .iter_mut()
        .find(|document| document.namespace == "providers.model")
        .expect("provider settings")
        .value_json = json!({
            "harness": { "address": "http://localhost:9810" }, "providers": [{
            "kind": "openai-compatible", "id": "primary", "enabled": true, "label": "Primary", "location": "local",
            "endpoint": primary_endpoint, "model": "primary-model", "authentication": "none"
        }, {
            "kind": "openai-compatible", "id": "fallback", "enabled": true, "label": "Fallback", "location": "local",
            "endpoint": fallback_endpoint, "model": "fallback-model", "authentication": "none"
        }], "reasoningEffort": "medium"});
    let route = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.tasks")
        .expect("route settings");
    route.value_json["conversationRespond"]["source"] = json!("provider");
    route.value_json["conversationRespond"]["primaryProviderId"] = json!("primary");
    route.value_json["conversationRespond"]["fallbackProviderIds"] = json!(["fallback"]);
    route.value_json["voiceSpeak"]["source"] = json!("harness");
    route.value_json["voiceSpeak"]["providerId"] = Value::Null;
    save_settings_documents_to_connection(
        &mut state.sqlite_writer.lock().expect("database lock"),
        &documents,
    )
    .expect("settings save");
    let input = StartTurnInput {
        run_id: "run-fallback".to_string(),
        conversation_id: conversation.id,
        content: "test fallback".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    let events_for_channel = events.clone();
    let completed_states = Arc::new(Mutex::new(Vec::<String>::new()));
    let completed_states_for_channel = completed_states.clone();
    let delta_states = Arc::new(Mutex::new(Vec::<i64>::new()));
    let delta_states_for_channel = delta_states.clone();
    let database_for_channel = state.sqlite_writer.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            if value.contains("\"type\":\"messageCompleted\"") {
                let status = database_for_channel
                    .lock()
                    .expect("database lock")
                    .query_row(
                        "SELECT status FROM runtime_runs WHERE id='run-fallback'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("committed run is readable from callback");
                completed_states_for_channel
                    .lock()
                    .expect("completed state lock")
                    .push(status);
            } else if value.contains("\"type\":\"delta\"") {
                let output_started = database_for_channel
                    .lock()
                    .expect("database lock")
                    .query_row(
                        "SELECT output_started FROM provider_sessions
                             WHERE runtime_run_id='run-fallback' AND provider_id='fallback'",
                        [],
                        |row| row.get(0),
                    )
                    .expect("committed output state is readable from callback");
                delta_states_for_channel
                    .lock()
                    .expect("delta state lock")
                    .push(output_started);
            }
            events_for_channel.lock().expect("event lock").push(value);
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
    .expect("fallback completes");
    primary_server.await.expect("primary server joins");
    fallback_server.await.expect("fallback server joins");
    let messages = list_messages_from_connection(
        &state.sqlite_writer.lock().expect("database lock"),
        &input.conversation_id,
    )
    .expect("messages load");
    assert_eq!(
        messages.last().expect("assistant message").content,
        "fallback ok"
    );
    let projected = events.lock().expect("event lock").join("\n");
    assert!(projected.contains("providerFailed"));
    assert!(projected.contains("messageCompleted"));
    assert_eq!(projected.matches("\"kind\":\"context-window\"").count(), 1);
    assert!(projected.contains("Context green:"));
    assert_eq!(
        *completed_states.lock().expect("completed state lock"),
        vec!["completed"]
    );
    assert_eq!(*delta_states.lock().expect("delta state lock"), vec![1]);
}
#[tokio::test]
pub(super) async fn partial_provider_stream_never_reaches_the_fallback_provider() {
    use std::io::ErrorKind;
    use std::net::TcpListener;

    let (primary_endpoint, _, primary_server) =
        spawn_llm_http_fixture(vec![LlmHttpStep::Delta("partial"), LlmHttpStep::Disconnect]).await;

    let fallback = TcpListener::bind("127.0.0.1:0").expect("fallback binds");
    let fallback_address = fallback.local_addr().expect("fallback address");
    fallback
        .set_nonblocking(true)
        .expect("fallback becomes nonblocking");
    let fallback_hit = Arc::new(AtomicBool::new(false));
    let fallback_hit_for_server = fallback_hit.clone();
    let fallback_server = thread::spawn(move || {
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_millis(500) {
            match fallback.accept() {
                Ok(_) => {
                    fallback_hit_for_server.store(true, Ordering::SeqCst);
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
            "harness": { "address": "http://localhost:9810" }, "providers": [{
            "kind": "openai-compatible", "id": "partial-primary", "enabled": true, "label": "Partial primary", "location": "local",
            "endpoint": primary_endpoint, "model": "primary-model", "authentication": "none"
        }, {
            "kind": "openai-compatible", "id": "forbidden-fallback", "enabled": true, "label": "Forbidden fallback", "location": "local",
            "endpoint": format!("http://{fallback_address}/v1"), "model": "fallback-model", "authentication": "none"
        }], "reasoningEffort": "medium"});
    let route = documents
        .iter_mut()
        .find(|document| document.namespace == "routing.tasks")
        .expect("routing settings");
    route.value_json["conversationRespond"]["source"] = json!("provider");
    route.value_json["conversationRespond"]["primaryProviderId"] = json!("partial-primary");
    route.value_json["conversationRespond"]["fallbackProviderIds"] = json!(["forbidden-fallback"]);
    route.value_json["voiceSpeak"]["source"] = json!("harness");
    route.value_json["voiceSpeak"]["providerId"] = Value::Null;
    save_settings_documents_to_connection(
        &mut state.sqlite_writer.lock().expect("database lock"),
        &documents,
    )
    .expect("settings save");
    let input = StartTurnInput {
        run_id: "run-partial".to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "partial test".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    execute_turn(
        &state,
        &input,
        &channel,
        Arc::new(RunCancellation::default()),
        None,
    )
    .await
    .expect_err("partial stream fails the turn");
    primary_server.await.expect("primary server joins");
    fallback_server.join().expect("fallback server joins");

    assert!(!fallback_hit.load(Ordering::SeqCst));
    let connection = state.sqlite_writer.lock().expect("database lock");
    let assistant_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_messages
                 WHERE conversation_id=?1 AND role='assistant'",
            params![PRIMARY_CONVERSATION_ID],
            |row| row.get(0),
        )
        .expect("assistant count reads");
    let (session_count, failure_reason): (i64, String) = connection
        .query_row(
            "SELECT COUNT(*), MAX(failure_reason) FROM provider_sessions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("provider session reads");
    assert_eq!(assistant_count, 0);
    assert_eq!(session_count, 1);
    assert_eq!(failure_reason, "network");
}
#[test]
pub(super) fn codex_thread_mapping_survives_database_reopen() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("threads.sqlite3");
    let connection = Connection::open(&path).expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
        .execute(
            "INSERT INTO conversations(id, title, task_mode, created_at, updated_at)
                 VALUES ('coding-conversation', NULL, 'coding', 'now', 'now')",
            [],
        )
        .expect("conversation inserts");
    let state = app_state(connection);
    persist_codex_thread(
        &state,
        "coding-conversation",
        "thread-persisted",
        "gpt-test",
        directory.path(),
    )
    .expect("thread persists");
    drop(state);

    let reopened = Connection::open(path).expect("database reopens");
    initialize_database(&reopened).expect("database reinitializes");
    let thread_id: String = reopened
        .query_row(
            "SELECT thread_id FROM codex_threads WHERE conversation_id = 'coding-conversation'",
            [],
            |row| row.get(0),
        )
        .expect("thread reloads");
    assert_eq!(thread_id, "thread-persisted");
}
#[test]
pub(super) fn audio_resampling_and_cancellation_are_bounded_and_idempotent() {
    let input = (0..48_000)
        .map(|index| (index as f32 / 48_000.0) * 2.0 - 1.0)
        .collect::<Vec<_>>();
    let output = voice::network_asr::resample_pcm(&input, 48_000, 16_000);
    assert_eq!(output.len(), 16_000);
    assert!(output.iter().all(|sample| (-1.0..=1.0).contains(sample)));
    let cancellation = RunCancellation::default();
    cancellation.cancel();
    cancellation.cancel();
    assert!(cancellation.is_cancelled());
}
#[cfg(target_os = "macos")]
#[test]
pub(super) fn macos_system_tts_runtime_is_available() {
    assert!(Command::new("say")
        .args(["-v", "?"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("system say command starts")
        .success());
}
