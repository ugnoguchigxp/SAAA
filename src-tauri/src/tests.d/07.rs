#[tokio::test]
async fn rr_15_asr_tool_tts_reconnect_e2e_and_rr_38_normal_provider_turn_specialist_returns_to_parent(
) {
    let (author_endpoint, author_requests, author_server) = spawn_llm_http_sequence_fixture(vec![
        vec![LlmHttpStep::Delta("private draft"), LlmHttpStep::Complete],
        vec![
            LlmHttpStep::Delta("final answer after host result"),
            LlmHttpStep::Complete,
        ],
    ])
    .await;
    let (specialist_endpoint, specialist_requests, specialist_server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta(
            r#"{"toolName":"tools_search","arguments":{"intent":"find the requested record","limit":1}}"#,
        ),
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
            "kind":"openai-compatible","id":"author-provider","enabled":true,
            "label":"Author","location":"local","endpoint":author_endpoint,
            "model":"author-model","authentication":"none"
        }, {
            "kind":"openai-compatible","id":"specialist-provider","enabled":true,
            "label":"Specialist","location":"local","endpoint":specialist_endpoint,
            "model":"specialist-model","authentication":"none"
        }],
        "reasoningEffort":"medium"
    });
    documents
        .iter_mut()
        .find(|document| document.namespace == "routing.roles")
        .expect("role policy")
        .value_json = specialist_role_policy("author-provider", "specialist-provider");
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

    let input = StartTurnInput {
        run_id: "rr-specialist-run".into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: "find the requested record".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: Some("asr-final-1".into()),
        scope_refs: Vec::new(),
        input_origin: "voice".into(),
        presentation_mode: "visual-and-spoken".into(),
    };
    let ui_events = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = ui_events.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            captured.lock().expect("event lock").push(value);
        }
        Ok(())
    });
    let event_hub = crate::runtime::event_hub::TurnEventHub::new(
        channel,
        crate::voice::streaming_tts::runtime::StreamingSpeechRuntime::default(),
        true,
    )
    .with_routing_speech(state.sqlite_writer.clone());
    assert_eq!(
        crate::voice_behavior::begin_turn_speech_policy(&state, &input)
            .expect("voice presentation policy"),
        (true, true)
    );
    execute_turn(
        &state,
        &input,
        &event_hub,
        Arc::new(RunCancellation::default()),
        None,
    )
    .await
    .expect("specialist turn completes");
    crate::voice_behavior::end_run(&state, &input.run_id);
    tokio::time::sleep(Duration::from_millis(40)).await;
    author_server.await.expect("author server");
    specialist_server.await.expect("specialist server");
    assert_eq!(author_requests.lock().expect("author requests").len(), 2);
    assert_eq!(
        specialist_requests
            .lock()
            .expect("specialist requests")
            .len(),
        1
    );
    assert!(specialist_requests.lock().expect("specialist request")[0]
        .contains("You cannot answer the user"));
    assert!(author_requests.lock().expect("author request")[1].contains("host-tool-result"));
    {
        let database = state.sqlite_writer.lock().expect("database lock");
        let statuses = database
            .prepare("SELECT purpose||':'||status FROM rr_steps WHERE root_id=?1 ORDER BY ordinal")
            .expect("steps")
            .query_map([&input.run_id], |row| row.get::<_, String>(0))
            .expect("rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("statuses");
        assert_eq!(
            statuses,
            vec![
                "respond:succeeded",
                "tool_specialist:succeeded",
                "respond:succeeded"
            ]
        );
        assert_eq!(
            database
            .query_row(
                "SELECT content FROM conversation_messages WHERE role='assistant' ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("final message"),
            "final answer after host result"
        );
        assert_eq!(
            database
            .query_row(
                "SELECT count(*) FROM rr_tool_links WHERE root_id=?1 AND dispatch_state='settled'",
                [&input.run_id],
                |row| row.get::<_, i64>(0),
            )
            .expect("tool ledger"),
            1
        );
        assert_eq!(
            database
            .query_row(
                "SELECT origin||':'||source_id||':'||disposition FROM rr_inputs WHERE root_id=?1",
                [&input.run_id],
                |row| row.get::<_, String>(0),
            )
            .expect("ASR receipt"),
            "voice:asr-final-1:accepted"
        );
        assert_eq!(
            database
                .query_row(
                    "SELECT kind||':'||status FROM rr_speech WHERE root_id=?1",
                    [&input.run_id],
                    |row| row.get::<_, String>(0),
                )
                .expect("TTS intent"),
            "final:queued"
        );
        let replay = crate::role_routing::ipc::replay(&database, &input.run_id, 0)
            .expect("reconnect event replay");
        assert!(replay.iter().any(|event| event.kind == "answer_committed"));
        assert!(ui_events
            .lock()
            .expect("UI events")
            .iter()
            .any(|event| event.contains("\"type\":\"messageCompleted\"")));
    }

    crate::runtime::event_hub::RuntimeEventSender::send(
        &event_hub,
        RuntimeEvent::SpeechFailed {
            run_id: input.run_id.clone(),
            message: "injected mock TTS failure".into(),
            recovery: "use visual answer".into(),
        },
    )
    .expect("TTS failure event remains deliverable");
    tokio::time::sleep(Duration::from_millis(40)).await;
    let database = state.sqlite_writer.lock().expect("database lock");
    assert_eq!(
        database
            .query_row(
                "SELECT status FROM rr_speech WHERE root_id=?1",
                [&input.run_id],
                |row| row.get::<_, String>(0),
            )
            .expect("failed TTS intent"),
        "failed"
    );
    assert_eq!(
        database
            .query_row(
                "SELECT r.phase||':'||m.content FROM rr_roots r JOIN conversation_messages m ON m.id=r.result_message_id WHERE r.root_id=?1",
                [&input.run_id],
                |row| row.get::<_, String>(0),
            )
            .expect("visual answer survives TTS failure"),
        "completed:final answer after host result"
    );
}
#[tokio::test]
async fn rr_23_challenge_starts_new_root_without_reopening_completed_root() {
    let (endpoint, requests, server) = spawn_llm_http_sequence_fixture(vec![
        vec![LlmHttpStep::Delta("first answer"), LlmHttpStep::Complete],
        vec![
            LlmHttpStep::Delta("reconsidered answer"),
            LlmHttpStep::Complete,
        ],
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
            "kind":"openai-compatible","id":"author-provider","enabled":true,
            "label":"Author","location":"local","endpoint":endpoint,
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
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    for (run_id, content) in [
        ("rr-feedback-original", "give an answer"),
        (
            "rr-feedback-challenge",
            "その回答は本当に正しいですか。根拠を再考して",
        ),
    ] {
        execute_turn(
            &state,
            &StartTurnInput {
                run_id: run_id.into(),
                conversation_id: PRIMARY_CONVERSATION_ID.into(),
                content: content.into(),
                workspace_path: None,
                retry_input_message_id: None,
                source_id: None,
                scope_refs: Vec::new(),
                input_origin: "text".into(),
                presentation_mode: "visual".into(),
            },
            &channel,
            Arc::new(RunCancellation::default()),
            None,
        )
        .await
        .expect("turn completes");
    }
    server.await.expect("provider server");
    assert_eq!(requests.lock().expect("requests").len(), 2);
    let database = state.sqlite_writer.lock().expect("database lock");
    for root_id in ["rr-feedback-original", "rr-feedback-challenge"] {
        assert_eq!(
            database
                .query_row(
                    "SELECT phase FROM rr_roots WHERE root_id=?1",
                    [root_id],
                    |row| row.get::<_, String>(0),
                )
                .expect("root"),
            "completed"
        );
    }
    let (target_root, source_root): (String, String) = database
        .query_row(
            "SELECT f.target_root_id, i.root_id
             FROM rr_feedback f
             JOIN rr_inputs i ON i.message_id=f.source_message_id
             WHERE f.kind='answer_challenge'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("challenge feedback");
    assert_eq!(target_root, "rr-feedback-original");
    assert_eq!(source_root, "rr-feedback-challenge");
}
#[tokio::test]
async fn rr_35_shadow_no_second_call_on_the_real_turn_path() {
    let (endpoint, requests, server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delta("shadow-safe answer"),
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
            "kind":"openai-compatible","id":"author-provider","enabled":true,
            "label":"Author","location":"local","endpoint":endpoint,
            "model":"author-model","authentication":"none"
        }],
        "reasoningEffort":"medium"
    });
    let mut policy = single_role_policy("author-provider");
    policy["selection"]["mode"] = json!("shadow");
    policy["selection"]["shadowArtifactId"] = json!("shadow-artifact");
    documents
        .iter_mut()
        .find(|document| document.namespace == "routing.roles")
        .expect("role policy")
        .value_json = policy;
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
    state
        .sqlite_writer
        .write(|connection| {
            let candidate_ids = vec!["direct-response".to_string()];
            let fingerprint = crate::adaptive_improvement::fingerprint_for(&candidate_ids);
            connection.execute("INSERT INTO rr_datasets(id,upper_seq,feature_version,labeler_version,manifest_json,state,created_at_ms) VALUES('shadow-dataset',0,'rr-features-v1','rr-labeler-v1','{}','ready',1)", []).map_err(|error| error.to_string())?;
            connection.execute("INSERT INTO rr_ranker_artifacts(id,dataset_id,algorithm,feature_version,candidate_fingerprint,weights_json,metrics_json,digest,state,created_at_ms) VALUES('shadow-artifact','shadow-dataset','empirical-v1','rr-features-v1',?1,'{\"direct-response\":0.9}','{}','fixture','shadow',1)", [fingerprint]).map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("shadow fixture");
    let input = StartTurnInput {
        run_id: "rr-shadow-run".into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: "one provider call only".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
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
    .expect("turn completes");
    server.await.expect("provider server");
    assert_eq!(requests.lock().expect("requests").len(), 1);
    let database = state.sqlite_writer.lock().expect("database lock");
    assert_eq!(
        database
            .query_row(
                "SELECT d.selected_id||':'||s.rules_id||':'||s.recommended_id FROM rr_decisions d JOIN rr_shadow_observations s ON s.decision_id=d.id WHERE d.root_id='rr-shadow-run'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("shadow receipt"),
        "direct-response:direct-response:direct-response"
    );
}
#[test]
fn rr_29_provider_before_classifier_raises_durable_barrier_and_queues_follow_up() {
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
    let input = |run_id: &str, content: &str| StartTurnInput {
        run_id: run_id.into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: content.into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    prepare_runtime_run(&state, &input("rr-active", "initial request")).expect("first receipt");
    prepare_runtime_run(
        &state,
        &input("rr-follow-up", "include the newly supplied detail"),
    )
    .expect("follow-up receipt");

    let database = state.sqlite_writer.lock().expect("database lock");
    let states = database
        .prepare("SELECT root_id||':'||phase FROM rr_roots ORDER BY started_at_ms,root_id")
        .expect("statement")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("states");
    assert_eq!(states, vec!["rr-active:draining", "rr-follow-up:queued"]);
    assert_eq!(
        database
            .query_row(
                "SELECT kind FROM rr_events WHERE root_id='rr-active' ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0)
            )
            .expect("barrier event"),
        "input_barrier"
    );
    let barrier_at: i64 = database
        .query_row(
            "SELECT created_at_ms FROM rr_events WHERE root_id='rr-active' AND kind='input_barrier'",
            [],
            |row| row.get(0),
        )
        .expect("barrier time");
    drop(database);
    assert_eq!(
        crate::runtime::turns::expire_stale_input_barrier(
            &mut state.sqlite_writer.lock().expect("database lock"),
            PRIMARY_CONVERSATION_ID,
            barrier_at + 1_500,
        )
        .expect("expire barrier")
        .as_deref(),
        Some("rr-active")
    );
    let database = state.sqlite_writer.lock().expect("database lock");
    let states = database
        .prepare("SELECT root_id||':'||phase FROM rr_roots ORDER BY started_at_ms,root_id")
        .expect("statement")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("states");
    assert_eq!(states, vec!["rr-active:failed", "rr-follow-up:responding"]);
}
