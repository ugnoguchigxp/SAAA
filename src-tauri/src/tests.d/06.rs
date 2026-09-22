#[cfg(not(coverage))]
#[test]
#[ignore = "requires a local Codex runtime, authentication, and network access"]
fn codex_live_read_only_turn_completes() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let outcome = run_codex_turn_process(
        "run-live-smoke",
        "Reply with exactly SAAA_LIVE_OK. Do not use tools.",
        workspace.path(),
        "",
        None,
        120_000,
        &events,
        &RunCancellation::default(),
    )
    .expect("live Codex turn succeeds");
    assert!(outcome.content.contains("SAAA_LIVE_OK"));
    assert_eq!(
        fs::read_dir(workspace.path())
            .expect("workspace remains readable")
            .count(),
        0,
        "read-only Codex turn must not create workspace files"
    );
}
#[cfg(not(coverage))]
#[test]
#[ignore = "requires a local Codex runtime, authentication, and network access"]
fn codex_live_read_only_turn_cancels_after_turn_start() {
    let workspace = tempfile::tempdir().expect("temporary workspace");
    let cancellation = Arc::new(RunCancellation::default());
    let cancellation_for_events = cancellation.clone();
    let events: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |_| {
        cancellation_for_events.cancel();
        Ok(())
    });
    let failure = run_codex_turn_process(
        "run-live-cancel",
        "Explain the read-only runtime lifecycle in detail. Do not use tools.",
        workspace.path(),
        "",
        None,
        120_000,
        &events,
        &cancellation,
    )
    .expect_err("live Codex turn is cancelled after turn/start");
    assert_eq!(
        failure.code,
        runtime::contracts::RunFailureCode::UserCancelled
    );
    assert_eq!(
        fs::read_dir(workspace.path())
            .expect("workspace remains readable")
            .count(),
        0,
        "cancelled read-only Codex turn must not create workspace files"
    );
}
fn world_body_history(conversation_id: &str) -> Vec<ConversationMessage> {
    vec![
        ConversationMessage {
            parts: None,
            id: "context-system".into(),
            conversation_id: conversation_id.into(),
            role: "system".into(),
            content: "policy".into(),
            created_at: "system".into(),
        },
        ConversationMessage {
            parts: None,
            id: "context-world".into(),
            conversation_id: conversation_id.into(),
            role: "assistant".into(),
            content: "WORLD_BLOCK_PRESENT".into(),
            created_at: "1".into(),
        },
        ConversationMessage {
            parts: None,
            id: "context-current".into(),
            conversation_id: conversation_id.into(),
            role: "user".into(),
            content: "hello".into(),
            created_at: "2".into(),
        },
    ]
}
async fn run_world_body_case(valid: bool, run_id: &str, session_provider_id: &str) -> (Value, i64) {
    let (endpoint, captures, server) =
        spawn_llm_http_fixture(vec![LlmHttpStep::Delta("ok"), LlmHttpStep::Complete]).await;
    let connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    let state = app_state(connection);
    let input = StartTurnInput {
        run_id: run_id.to_string(),
        conversation_id: PRIMARY_CONVERSATION_ID.to_string(),
        content: "hello".to_string(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    prepare_runtime_run(&state, &input).expect("runtime prepares");
    let session_id = begin_test_provider_session(
        &state,
        &input.run_id,
        session_provider_id,
        "openai-compatible",
    )
    .expect("provider session starts");
    let world = crate::runtime::context::world::turn::WorldLive::for_test(
        valid,
        "WORLD_BLOCK_PRESENT",
        Some("WORLD_BLOCK_ABSENT"),
    );
    let history = world_body_history(&input.conversation_id);
    // The record must list the World source exactly when the body carries its block.
    let world_candidate = crate::runtime::context::source::Candidate::untrusted(
        "world-candidate".to_string(),
        crate::runtime::context::world::source::WORLD_KIND,
        Vec::new(),
        crate::runtime::context::source::Requirement::May,
        "world-source".to_string(),
        1,
        0,
        "world".to_string(),
    );
    let context_sources = std::slice::from_ref(&world_candidate);
    let provider = OpenAiCompatibleProviderSettings {
        endpoint,
        ..direct_provider(session_provider_id, "local")
    };
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let outcome = stream_model_provider(
        &provider,
        &history,
        5_000,
        ModelStreamContext {
            reasoning_effort: providers::DEFAULT_CONVERSATION_REASONING_EFFORT,
            max_output_tokens: providers::completion::DEFAULT_MAX_OUTPUT_TOKENS,
            input: &input,
            on_event: &channel,
            cancellation: Arc::new(RunCancellation::default()),
            context_health: "green",
            context_sources,
            context_omissions: &[],
            output_persistence: Some(ProviderOutputPersistence {
                state: &state,
                session_id: &session_id,
                world: Some(&world),
            }),
        },
    )
    .await;
    server.await.expect("fixture joins");
    let ProviderAttemptOutcome::Completed { .. } = outcome else {
        panic!("provider stream should complete");
    };
    let captures = captures.lock().expect("capture lock");
    assert_eq!(captures.len(), 1, "one provider request");
    let body = serde_json::from_str(&captures[0]).expect("request JSON");
    drop(captures);
    let selected_world: i64 = state
        .sqlite_readers
        .read(|connection| {
            connection
                .query_row(
                    "SELECT COALESCE(MAX(selected),0) FROM context_generation_inputs \
                     WHERE source_kind=?1",
                    [crate::runtime::context::world::source::WORLD_KIND],
                    |row| row.get(0),
                )
                .map_err(crate::database_error)
        })
        .expect("world input record reads");
    (body, selected_world)
}
#[tokio::test]
async fn expired_world_block_is_removed_from_the_sent_provider_body_and_record() {
    let (body, selected_world) =
        run_world_body_case(false, "run-world-expired", "world-expired-fixture").await;
    let rendered = body["messages"].to_string();
    assert!(
        !rendered.contains("WORLD_BLOCK_PRESENT"),
        "an expired World block must not reach the provider body"
    );
    assert!(
        rendered.contains("WORLD_BLOCK_ABSENT"),
        "the World-free rendering must reach the provider body"
    );
    assert_eq!(
        selected_world, 0,
        "an expired World must not be recorded as selected"
    );
}
#[tokio::test]
async fn valid_world_block_is_sent_to_the_provider_body_and_record() {
    let (body, selected_world) =
        run_world_body_case(true, "run-world-valid", "world-valid-fixture").await;
    let rendered = body["messages"].to_string();
    assert!(
        rendered.contains("WORLD_BLOCK_PRESENT"),
        "a current World block must reach the provider body"
    );
    assert!(
        !rendered.contains("WORLD_BLOCK_ABSENT"),
        "the World-free rendering must not replace a current World block"
    );
    assert_eq!(
        selected_world, 1,
        "a current World must be recorded as selected"
    );
}
fn two_step_role_policy(front_provider: &str, reason_provider: &str) -> Value {
    json!({
        "schemaVersion": 1, "enabled": true,
        "actors": [{
            "id":"front","label":"Front","aliases":[],"transport":"provider",
            "providerId":front_provider,"model":null,"location":"local",
            "resourceGroup":"front","maxInputBytes":16384,"capabilities":["reason"]
        }, {
            "id":"reason","label":"Reason","aliases":[],"transport":"provider",
            "providerId":reason_provider,"model":null,"location":"local",
            "resourceGroup":"reason","maxInputBytes":16384,"capabilities":["reason"]
        }],
        "roles":{"frontend":"front","reasoner":"reason","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},
        "recipes":[{"id":"ack-then-reason","action":"respond","roles":["frontend","reasoner"],"enabled":true}],
        "limits":{"maxReasoningSteps":2,"maxToolCalls":0,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":0,"maxAutomaticSwitches":0,"maxEstimatedCostMicros":null},
        "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
        "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
        "premiumApproval":"never",
        "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false},
        "adaptiveImprovement":{"enabled":false,"providerRecipe":false,"tool":false,"plan":false,"notification":false}
    })
}
fn reviewed_role_policy(author_provider: &str, reviewer_provider: &str) -> Value {
    json!({
        "schemaVersion": 1, "enabled": true,
        "actors": [{
            "id":"author","label":"Author","aliases":[],"transport":"provider",
            "providerId":author_provider,"model":null,"location":"local",
            "resourceGroup":"author","maxInputBytes":65536,"capabilities":["reason"]
        }, {
            "id":"reviewer","label":"Reviewer","aliases":[],"transport":"provider",
            "providerId":reviewer_provider,"model":null,"location":"local",
            "resourceGroup":"reviewer","maxInputBytes":65536,"capabilities":["reason"]
        }],
        "roles":{"frontend":null,"reasoner":"author","advanced":null,"reviewer":"reviewer","premium":null,"toolSpecialist":null},
        "recipes":[{"id":"reviewed-response","action":"respond","roles":["reasoner","reviewer","reasoner"],"enabled":true}],
        "limits":{"maxReasoningSteps":3,"maxToolCalls":0,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":1,"maxAutomaticSwitches":0,"maxEstimatedCostMicros":null},
        "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
        "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
        "premiumApproval":"never",
        "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false},
        "adaptiveImprovement":{"enabled":false,"providerRecipe":false,"tool":false,"plan":false,"notification":false}
    })
}
fn specialist_role_policy(author_provider: &str, specialist_provider: &str) -> Value {
    json!({
        "schemaVersion": 1, "enabled": true,
        "actors": [{
            "id":"author","label":"Author","aliases":[],"transport":"provider",
            "providerId":author_provider,"model":null,"location":"local",
            "resourceGroup":"author","maxInputBytes":65536,"capabilities":["reason"]
        }, {
            "id":"specialist","label":"Specialist","aliases":[],"transport":"provider",
            "providerId":specialist_provider,"model":null,"location":"local",
            "resourceGroup":"specialist","maxInputBytes":65536,"capabilities":["reason"]
        }],
        "roles":{"frontend":null,"reasoner":"author","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":"specialist"},
        "recipes":[{"id":"specialist-response","action":"respond","roles":["reasoner","tool_specialist","reasoner"],"enabled":true}],
        "limits":{"maxReasoningSteps":3,"maxToolCalls":1,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":0,"maxAutomaticSwitches":0,"maxEstimatedCostMicros":null},
        "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
        "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
        "premiumApproval":"never",
        "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false},
        "adaptiveImprovement":{"enabled":false,"providerRecipe":false,"tool":false,"plan":false,"notification":false}
    })
}
fn single_role_policy(provider_id: &str) -> Value {
    json!({
        "schemaVersion": 1, "enabled": true,
        "actors": [{
            "id":"author","label":"Author","aliases":[],"transport":"provider",
            "providerId":provider_id,"model":null,"location":"local",
            "resourceGroup":"author","maxInputBytes":16384,"capabilities":["reason"]
        }],
        "roles":{"frontend":null,"reasoner":"author","advanced":null,"reviewer":null,"premium":null,"toolSpecialist":null},
        "recipes":[{"id":"direct-response","action":"respond","roles":["reasoner"],"enabled":true}],
        "limits":{"maxReasoningSteps":1,"maxToolCalls":0,"rootTimeoutMs":180000,"stepTimeoutMs":60000,"frontendTimeoutMs":1200,"classificationTimeoutMs":1500,"maxQueuedInputs":4,"maxReviewRounds":0,"maxAutomaticSwitches":0,"maxEstimatedCostMicros":null},
        "speech":{"mode":"author_verbatim","ackDelayMs":250,"maxAckChars":80,"progressMinIntervalMs":15000,"maxProgressPerRoot":2},
        "selection":{"mode":"rules","shadowArtifactId":null,"classificationMinConfidence":0.85,"weights":{"quality":0.6,"latency":0.25,"cost":0.15},"switchMargin":0.15},
        "premiumApproval":"never",
        "learning":{"enabled":false,"localStart":"02:00","localEnd":"05:00","idleSeconds":300,"maxRunSeconds":600,"batchSize":100,"allowLocalLabeler":false},
        "adaptiveImprovement":{"enabled":false,"providerRecipe":false,"tool":false,"plan":false,"notification":false}
    })
}
#[tokio::test]
async fn rr_25_normal_turn_author_review_revise() {
    let (author_endpoint, author_requests, author_server) = spawn_llm_http_sequence_fixture(vec![
        vec![LlmHttpStep::Delta("author draft"), LlmHttpStep::Complete],
        vec![LlmHttpStep::Delta("revised final"), LlmHttpStep::Complete],
    ])
    .await;
    let (reviewer_endpoint, reviewer_requests, reviewer_server) = spawn_llm_http_fixture(vec![
        LlmHttpStep::Delay(50),
        LlmHttpStep::Delta(
            r#"{"issues":[{"kind":"logic","claim":"the conclusion does not follow","severity":"major","code":"non-sequitur","evidenceRef":"rr-output-rr-step-rr-reviewed-run-0","verdict":"verified"}]}"#,
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
            "kind":"openai-compatible","id":"reviewer-provider","enabled":true,
            "label":"Reviewer","location":"local","endpoint":reviewer_endpoint,
            "model":"reviewer-model","authentication":"none"
        }],
        "reasoningEffort":"medium"
    });
    documents
        .iter_mut()
        .find(|document| document.namespace == "routing.roles")
        .expect("role policy")
        .value_json = reviewed_role_policy("author-provider", "reviewer-provider");
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
    let verifier_db = state.sqlite_writer.clone();
    let verifier = tokio::spawn(async move {
        for _ in 0..1_000 {
            let changed = verifier_db
                .write(|connection| {
                    connection
                        .execute(
                            "UPDATE rr_outputs SET payload_json=json_set(payload_json,'$.hostVerification','verified','$.verifierVersion','fixture-v1') WHERE id='rr-output-rr-step-rr-reviewed-run-0'",
                            [],
                        )
                        .map_err(|error| error.to_string())
                })
                .expect("verification update");
            if changed == 1 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("draft output was not produced");
    });
    let input = StartTurnInput {
        run_id: "rr-reviewed-run".into(),
        conversation_id: PRIMARY_CONVERSATION_ID.into(),
        content: "review this answer path".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    };
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(|_| Ok(()));
    let execution = execute_turn(
        &state,
        &input,
        &channel,
        Arc::new(RunCancellation::default()),
        None,
    )
    .await;
    if let Err(error) = execution {
        let database = state.sqlite_writer.lock().expect("database lock");
        let statuses = database
            .prepare(
                "SELECT ordinal,purpose,status FROM rr_steps WHERE root_id=?1 ORDER BY ordinal",
            )
            .expect("debug steps")
            .query_map([&input.run_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .expect("debug rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("debug statuses");
        let phase = database
            .query_row(
                "SELECT phase FROM rr_roots WHERE root_id=?1",
                [&input.run_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap_or_else(|_| "missing".into());
        let sessions = database
            .prepare("SELECT provider_id,status,failure_reason FROM provider_sessions ORDER BY started_at,id")
            .expect("debug sessions")
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .expect("debug session rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("debug session states");
        let reviewer_bodies = reviewer_requests.lock().expect("reviewer debug").clone();
        panic!("reviewed turn failed: {error:?}; phase={phase}; steps={statuses:?}; sessions={sessions:?}; reviewerBodies={reviewer_bodies:?}");
    }
    verifier.await.expect("verifier");
    author_server.await.expect("author server");
    reviewer_server.await.expect("reviewer server");
    assert_eq!(author_requests.lock().expect("author requests").len(), 2);
    assert_eq!(
        reviewer_requests.lock().expect("reviewer requests").len(),
        1
    );
    let review_body = reviewer_requests.lock().expect("reviewer requests")[0].clone();
    assert!(review_body.contains("author draft"));
    assert!(review_body.contains("rr-output-rr-step-rr-reviewed-run-0"));
    let revise_body = author_requests.lock().expect("author requests")[1].clone();
    assert!(revise_body.contains("verifiedIssues"));
    let database = state.sqlite_writer.lock().expect("database lock");
    assert_eq!(
        database
            .query_row(
                "SELECT content FROM conversation_messages WHERE role='assistant' ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("final message"),
        "revised final"
    );
    let statuses = database
        .prepare("SELECT status FROM rr_steps WHERE root_id=?1 ORDER BY ordinal")
        .expect("steps")
        .query_map([&input.run_id], |row| row.get::<_, String>(0))
        .expect("rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("statuses");
    assert_eq!(statuses, vec!["succeeded", "succeeded", "succeeded"]);
}
