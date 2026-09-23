use super::*;
#[tokio::test]
pub(super) async fn h12_mcp_correction_applies_to_the_matching_conversation_only() {
    let state = default_state();
    let mock = MockServer::start(state).await;
    let harness = Harness::new(&mock, vec![user_grant("search"), user_grant("other")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");

    // A normal conversation correction that prefers the other MCP tool. The message row makes the
    // correction a persisted user instruction, exactly like the conversation path.
    harness
        .writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                     VALUES ('msg-h12', 'conversation-d4', 'user', 'other を使って', '1')",
                    [],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("message");
    let context = harness.context().with_message(Some("msg-h12".to_string()));
    let parsed = crate::tool_selection::feedback::ParsedExtraction {
        scenario: scenario(),
        accepted: vec![crate::tool_selection::contracts::ExtractedFeedback {
            kind: crate::tool_selection::contracts::FeedbackKind::ToolChoice,
            decision_id: None,
            rejected_tool_id: None,
            preferred_tool_id: Some("other".to_string()),
            scope: crate::tool_selection::contracts::ScopeKind::Conversation,
            duration: crate::tool_selection::contracts::Duration::Persistent,
            evidence: crate::tool_selection::contracts::Evidence {
                start: 0,
                end: 5,
                text: "other".to_string(),
            },
            condition: crate::tool_selection::contracts::FeedbackCondition {
                operation: Some(crate::tool_selection::Operation::Search),
                object_type: Some(crate::tool_selection::ObjectType::Document),
                phase: None,
                input_kind: None,
            },
        }],
        rejected: Vec::new(),
    };
    harness
        .service
        .apply_parsed(&context, &parsed)
        .expect("apply correction");

    let other_tool = descriptors::tool_id("mcp-test", "other");
    harness.service.set_scenario(&context, scenario());
    let response = harness
        .service
        .search(&context, "search notes", 8)
        .await
        .expect("search");
    let applied = harness
        .service
        .decision_candidates(&response.decision_id)
        .expect("candidates")
        .into_iter()
        .find(|candidate| candidate.tool_id == other_tool)
        .expect("preferred candidate present");
    assert!(
        !applied.rule_ids.is_empty(),
        "the saved correction must reach the MCP search in its conversation"
    );

    // The same search in another conversation is untouched.
    harness
        .writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT OR IGNORE INTO conversations(id, title, task_mode, created_at, updated_at)
                     VALUES ('conversation-other', 'Other', 'conversation', '1', '1')",
                    [],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("other conversation");
    let other_context = RequestContext::new(&harness.principal, "conversation-other")
        .with_run(Some("run-other".to_string()));
    harness.service.set_scenario(&other_context, scenario());
    let other_response = harness
        .service
        .search(&other_context, "search notes", 8)
        .await
        .expect("other search");
    let untouched = harness
        .service
        .decision_candidates(&other_response.decision_id)
        .expect("candidates")
        .into_iter()
        .find(|candidate| candidate.tool_id == other_tool)
        .expect("candidate present");
    assert!(
        untouched.rule_ids.is_empty(),
        "a conversation correction must not leak into another conversation"
    );
}
#[tokio::test]
pub(super) async fn h12_correction_scope_distinguishes_operation_and_project() {
    let state = default_state();
    let mock = MockServer::start(state).await;
    let harness = Harness::new(&mock, vec![user_grant("search"), user_grant("other")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let principal = harness.principal.clone();
    let other_tool = descriptors::tool_id("mcp-test", "other");

    let insert_message = |id: &str| {
        harness
            .writer
            .write({
                let id = id.to_string();
                move |connection| {
                    connection
                        .execute(
                            "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                             VALUES (?1, 'conversation-d4', 'user', 'fixture', '1')",
                            rusqlite::params![id],
                        )
                        .map_err(|error| error.to_string())?;
                    Ok(())
                }
            })
            .expect("message");
    };
    let correction = |operation: &str, scope: crate::tool_selection::contracts::ScopeKind| {
        crate::tool_selection::feedback::ParsedExtraction {
            scenario: scenario(),
            accepted: vec![crate::tool_selection::contracts::ExtractedFeedback {
                kind: crate::tool_selection::contracts::FeedbackKind::ToolChoice,
                decision_id: None,
                rejected_tool_id: None,
                preferred_tool_id: Some("other".to_string()),
                scope,
                duration: crate::tool_selection::contracts::Duration::Persistent,
                evidence: crate::tool_selection::contracts::Evidence {
                    start: 0,
                    end: 5,
                    text: "other".to_string(),
                },
                condition: crate::tool_selection::contracts::FeedbackCondition {
                    operation: Some(crate::tool_selection::Operation::parse(operation)),
                    object_type: Some(crate::tool_selection::ObjectType::Document),
                    phase: None,
                    input_kind: None,
                },
            }],
            rejected: Vec::new(),
        }
    };
    let rule_ids_for = |decision_id: &str| -> Vec<String> {
        harness
            .service
            .decision_candidates(decision_id)
            .expect("candidates")
            .into_iter()
            .find(|candidate| candidate.tool_id == other_tool)
            .map(|candidate| candidate.rule_ids)
            .unwrap_or_default()
    };

    // A correction bound to a different operation must not apply to a search decision.
    insert_message("msg-h12-create");
    let create_context = harness
        .context()
        .with_message(Some("msg-h12-create".to_string()));
    harness
        .service
        .apply_parsed(
            &create_context,
            &correction(
                "create",
                crate::tool_selection::contracts::ScopeKind::Conversation,
            ),
        )
        .expect("apply create correction");
    harness.service.set_scenario(&create_context, scenario());
    let create_response = harness
        .service
        .search(&create_context, "search notes", 8)
        .await
        .expect("search");
    assert!(
        rule_ids_for(&create_response.decision_id).is_empty(),
        "a create-scoped correction must not affect a search"
    );

    // A project-scoped correction applies in its project and not in another.
    insert_message("msg-h12-project");
    let project_a = harness
        .context()
        .with_run(Some("run-a".to_string()))
        .with_message(Some("msg-h12-project".to_string()))
        .with_project(Some("A".to_string()));
    harness
        .service
        .apply_parsed(
            &project_a,
            &correction(
                "search",
                crate::tool_selection::contracts::ScopeKind::Project,
            ),
        )
        .expect("apply project correction");
    harness.service.set_scenario(&project_a, scenario());
    let response_a = harness
        .service
        .search(&project_a, "search notes", 8)
        .await
        .expect("search A");
    assert!(
        !rule_ids_for(&response_a.decision_id).is_empty(),
        "the project correction applies inside project A"
    );

    let project_b = RequestContext::new(&principal, "conversation-d4")
        .with_run(Some("run-b".to_string()))
        .with_project(Some("B".to_string()));
    harness.service.set_scenario(&project_b, scenario());
    let response_b = harness
        .service
        .search(&project_b, "search notes", 8)
        .await
        .expect("search B");
    assert!(
        rule_ids_for(&response_b.decision_id).is_empty(),
        "the project correction must not apply in project B"
    );
}
pub(super) async fn execute_candidate(
    service: &ToolSelectionService,
    context: &RequestContext,
    reference: &str,
) -> crate::tool_selection::ToolSelectionResult<crate::tool_selection::service::InvokeResponse> {
    let described = service
        .describe(context, reference, "contract", None)
        .expect("describe");
    let execution_ref = described.execution_ref.expect("execution ref");
    let cancellation = crate::RunCancellation::default();
    service
        .invoke(context, &execution_ref, &json!({ "q": "v" }), &cancellation)
        .await
}
#[tokio::test]
pub(super) async fn p03_same_name_routes_across_llang_and_two_mcp_sources() {
    let state_a = default_state();
    let state_b = default_state();
    let server_a = MockServer::start(state_a.clone()).await;
    let server_b = MockServer::start(state_b.clone()).await;
    let connection = Connection::open_in_memory().expect("in-memory");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let writer = Arc::new(SqliteWriter::from_connection(connection));
    writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT OR IGNORE INTO conversations(id, title, task_mode, created_at, updated_at)
                     VALUES ('conversation-d4', 'D4', 'conversation', '1', '1')",
                    [],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("conversation");
    let principal = crate::tool_selection::service::ensure_principal(&writer).expect("principal");
    let sources = McpSources {
        sources: vec![
            McpSourceSpec {
                id: "mcp-a".to_string(),
                url: server_a.url(),
                enabled: true,
                bearer_token_env: None,
                grants: vec![user_grant("search")],
            },
            McpSourceSpec {
                id: "mcp-b".to_string(),
                url: server_b.url(),
                enabled: true,
                bearer_token_env: None,
                grants: vec![user_grant("search")],
            },
        ],
    };
    let manager = McpManager::new(
        writer.clone(),
        principal.clone(),
        None,
        sources,
        Some(hash_embedding()),
        None,
    );
    manager.sync_source("mcp-a").await.expect("a");
    manager.sync_source("mcp-b").await.expect("b");

    // A third tool with the same display name, backed by the local L-Lang fixture.
    let entry = crate::tool_selection::catalog::CatalogEntry {
        tool_id: "llang-search".to_string(),
        backend_key: "search".to_string(),
        title: "search".to_string(),
        purpose: "Search notes locally for decision records.".to_string(),
        operations: vec!["search".to_string()],
        objects: vec!["document".to_string()],
        suitable: vec![],
        unsuitable: vec![],
        required_inputs: vec!["q".to_string()],
        input_schema: json!({ "type": "object", "properties": { "q": { "type": "string" } } }),
        output_schema: None,
        effect: "read",
        usage_pages: vec![],
        backend_binding: json!({
            "capabilityId": "llang-search",
            "revisionId": "llang-search-rev1",
            "packageHash": "p",
            "contractHash": "c",
            "catalogEpoch": 0,
            "inputFields": []
        }),
    };
    writer
        .write({
            let principal = principal.clone();
            move |connection| {
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(crate::database_error)?;
                crate::tool_selection::catalog::register_revision(
                    &transaction,
                    &principal,
                    "llang",
                    &entry,
                    "llang-search-rev1",
                    now_ms(),
                )
                .map_err(|error| error.code.as_str().to_string())?;
                repository::upsert_grant(
                    &transaction,
                    &principal,
                    "llang-search",
                    "user",
                    &principal,
                )
                .map_err(|error| error.to_string())?;
                repository::bump_epochs(&transaction, false, true, false)
                    .map_err(|error| error.to_string())?;
                transaction.commit().map_err(crate::database_error)?;
                Ok(())
            }
        })
        .expect("register llang");

    let router = Arc::new(BackendRouter::new(
        Arc::new(FixtureBackend::new()),
        Arc::new(McpBackend::new(manager.clone())),
        Arc::new(crate::records::backend::RecordsBackend::new(writer.clone())),
    ));
    let mut service = ToolSelectionService::new(
        writer.clone(),
        hash_embedding(),
        Arc::new(FixedReranker::new(&[])),
        Arc::new(UnconfiguredExtractor),
        router,
        f64::NEG_INFINITY,
    );
    service.set_mcp_manager(manager);
    service.set_discovery_configured(true);

    let context =
        RequestContext::new(&principal, "conversation-d4").with_run(Some("run-d4".into()));
    service.set_scenario(&context, scenario());
    let search = service
        .search(&context, "search notes", 8)
        .await
        .expect("search");
    let candidate_for = |source: &str| -> String {
        search
            .candidates
            .iter()
            .find(|candidate| candidate.source_id == source)
            .unwrap_or_else(|| panic!("missing {source} candidate"))
            .reference
            .clone()
    };

    let before_a = state_a.call_count.load(Ordering::SeqCst);
    let before_b = state_b.call_count.load(Ordering::SeqCst);

    let a = execute_candidate(&service, &context, &candidate_for("mcp-a"))
        .await
        .expect("invoke a");
    assert_eq!(
        a.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
    assert_eq!(state_a.call_count.load(Ordering::SeqCst), before_a + 1);
    assert_eq!(state_b.call_count.load(Ordering::SeqCst), before_b);

    let b = execute_candidate(&service, &context, &candidate_for("mcp-b"))
        .await
        .expect("invoke b");
    assert_eq!(
        b.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
    assert_eq!(state_b.call_count.load(Ordering::SeqCst), before_b + 1);
    assert_eq!(state_a.call_count.load(Ordering::SeqCst), before_a + 1);

    let local = execute_candidate(&service, &context, &candidate_for("llang"))
        .await
        .expect("invoke local");
    assert_eq!(
        local.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
    assert_eq!(state_a.call_count.load(Ordering::SeqCst), before_a + 1);
    assert_eq!(state_b.call_count.load(Ordering::SeqCst), before_b + 1);
}
#[tokio::test]
pub(super) async fn p03_profile_result_storage_limit_is_enforced_over_real_http() {
    let state = default_state();
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": "x".repeat(1_048_000) }],
        "isError": false,
    });
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");

    let mut stored = 0;
    let mut overflow = None;
    // A fresh run per call keeps each stored result inside the per-run reference/result bounds,
    // so the principal-wide profile limit (32 MiB) is what is measured.
    for index in 0..33 {
        let context = harness
            .context()
            .with_run(Some(format!("run-limit-{index}")));
        harness.service.set_scenario(&context, scenario());
        let search = harness
            .service
            .search(&context, "search notes", 1)
            .await
            .expect("search");
        let describe = harness
            .service
            .describe(&context, &search.candidates[0].reference, "contract", None)
            .expect("describe");
        let execution_ref = describe.execution_ref.expect("execution ref");
        let cancellation = crate::RunCancellation::default();
        let response = harness
            .service
            .invoke(
                &context,
                &execution_ref,
                &json!({ "q": "v" }),
                &cancellation,
            )
            .await
            .expect("invoke");
        assert_eq!(
            response.status,
            crate::tool_selection::backends::TechnicalStatus::Succeeded
        );
        match response.result_availability {
            Some("stored") => stored += 1,
            other => {
                overflow = Some((other, response.error_code));
                break;
            }
        }
    }
    assert_eq!(
        stored, 32,
        "32 results of ~1 MiB fill the 32 MiB profile budget"
    );
    assert_eq!(
        overflow,
        Some((Some("unavailable"), Some("result-storage-limit"))),
        "the next result must be reported as a storage limit, not a call failure"
    );
}
/// Minimal HTTP/1.1 server that answers every request with a 302 to `target`.
pub(crate) async fn redirect_server(target: String) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let mut buffer = [0_u8; 4096];
            let _ = socket.read(&mut buffer).await;
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    (format!("http://127.0.0.1:{port}/mcp"), handle)
}
