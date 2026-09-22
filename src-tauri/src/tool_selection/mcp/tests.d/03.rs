#[tokio::test]
async fn t07_cancel_before_send_makes_zero_calls() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let context = harness.context();
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
    cancellation.cancel();
    let result = harness
        .service
        .invoke(
            &context,
            &execution_ref,
            &json!({ "q": "value" }),
            &cancellation,
        )
        .await;
    assert!(
        result.is_err(),
        "a cancelled-before-send call is reported to the caller"
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn t08_large_result_is_paged_and_reassembles_exactly() {
    let state = default_state();
    let payload: String = "あ".repeat(40 * 1024); // 120 KiB of UTF-8
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": payload }],
        "isError": false,
    });
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "value" })).await;
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
    assert_eq!(response.result, None);
    assert_eq!(response.result_availability, Some("stored"));
    let result_ref = response.result_ref.clone().expect("result ref");
    let page_count = response.page_count.expect("page count");
    assert!(page_count > 1);

    let context = harness.context();
    let mut reassembled = String::new();
    for page in 0..page_count {
        let page = harness
            .service
            .describe_result(&context, &result_ref, page)
            .expect("page");
        reassembled.push_str(&page.text);
    }
    let expected = descriptors::canonical_json_string(&json!({
        "content": [{ "type": "text", "text": payload }],
        "isError": false,
    }));
    assert_eq!(reassembled, expected);
}
#[tokio::test]
async fn t08_result_scope_ttl_and_acl_are_enforced() {
    let state = default_state();
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": "x".repeat(40 * 1024) }],
    });
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "value" })).await;
    let result_ref = response.result_ref.expect("result ref");
    let context = harness.context();

    // Another run cannot read the stored result.
    let principal = harness.principal.clone();
    let other =
        RequestContext::new(&principal, "conversation-d4").with_run(Some("run-other".into()));
    assert!(harness
        .service
        .describe_result(&other, &result_ref, 0)
        .is_err());

    // An expired row is refused.
    harness
        .writer
        .write({
            let result_ref = result_ref.clone();
            move |connection| {
                connection
                    .execute(
                        "UPDATE tool_selection_mcp_results SET expires_at = 1 WHERE id = ?1",
                        rusqlite::params![result_ref],
                    )
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    assert!(harness
        .service
        .describe_result(&context, &result_ref, 0)
        .is_err());
}
#[tokio::test]
async fn t08_size_limit_keeps_the_scan_bounded() {
    let state = default_state();
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": "y".repeat(40 * 1024) }],
    });
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let tool_id = descriptors::tool_id("mcp-test", "search");
    let revision_id = harness
        .writer
        .read_serialized({
            let tool_id = tool_id.clone();
            move |c| {
                repository::tool_by_id(c, &tool_id)
                    .map_err(|e| e.to_string())?
                    .and_then(|tool| tool.current_revision_id)
                    .ok_or_else(|| "missing".to_string())
            }
        })
        .unwrap();
    // A payload above 1 MiB is reported as a size limit and is never inserted.
    let oversized = format!(
        "{{\"big\":\"{}\"}}",
        "z".repeat(super::MCP_RESULT_MAX_BYTES)
    );
    let outcome = harness
        .writer
        .write(move |connection| {
            let tx = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(crate::database_error)?;
            let outcome = results::store_result(
                &tx,
                "missing-invocation",
                PRINCIPAL,
                "conversation-d4",
                "run-d4",
                &tool_id,
                &revision_id,
                &"a".repeat(64),
                0,
                &oversized,
            )
            .map_err(|error| error.code.as_str().to_string())?;
            tx.commit().map_err(crate::database_error)?;
            Ok(outcome)
        })
        .unwrap();
    assert_eq!(outcome, results::StoreOutcome::SizeLimit);
}
// ---------------------------------------------------------------------------------------------
// T09 freshness
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn t09_stale_source_is_not_eligible() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    // Age the last success beyond the freshness window.
    harness
        .writer
        .write(|connection| {
            connection
                .execute(
                    "UPDATE tool_selection_mcp_sources SET last_success_at = ?1 WHERE source_id = 'mcp-test'",
                    rusqlite::params![now_ms() - super::MCP_SOURCE_STALE_AFTER_MILLIS - 1000],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    let eligible = harness
        .writer
        .read_serialized({
            let principal = harness.principal.clone();
            move |c| {
                repository::eligible_revisions(c, &principal, None, now_ms())
                    .map(|rows| rows.len())
                    .map_err(|e| e.to_string())
            }
        })
        .unwrap();
    assert_eq!(eligible, 0);

    // A remote invoke is refused before any HTTP call.
    let before = state.call_count.load(Ordering::SeqCst);
    let context = harness.context();
    harness.service.set_scenario(&context, scenario());
    // Search returns no candidate because the source is stale, so there is nothing to describe.
    let search = harness
        .service
        .search(&context, "search notes", 1)
        .await
        .expect("search");
    assert!(search.candidates.is_empty());
    assert_eq!(state.call_count.load(Ordering::SeqCst), before);
}
// ---------------------------------------------------------------------------------------------
// T10 same-name ambiguity
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn t10_same_name_in_two_sources_makes_a_name_only_correction_ambiguous() {
    // Two MCP sources both publish `search`. A name-only correction must not pick one.
    let state_a = default_state();
    let state_b = default_state();
    let server_a = MockServer::start(state_a).await;
    let server_b = MockServer::start(state_b).await;
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
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
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
    let id_a = descriptors::tool_id("mcp-a", "search");
    let id_b = descriptors::tool_id("mcp-b", "search");
    assert_ne!(id_a, id_b);

    // Both are authorized, so a name-only target is ambiguous and produces no rule.
    let context = RequestContext::new(&principal, "conversation-d4")
        .with_run(Some("run-d4".into()))
        .with_message(Some("msg-d4".into()));
    writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                     VALUES ('msg-d4', 'conversation-d4', 'user', 'fixture', '1')",
                    [],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    let parsed = crate::tool_selection::feedback::ParsedExtraction {
        scenario: scenario(),
        accepted: vec![crate::tool_selection::contracts::ExtractedFeedback {
            kind: crate::tool_selection::contracts::FeedbackKind::ToolChoice,
            decision_id: None,
            rejected_tool_id: Some("search".to_string()),
            preferred_tool_id: None,
            scope: crate::tool_selection::contracts::ScopeKind::Project,
            duration: crate::tool_selection::contracts::Duration::Persistent,
            evidence: crate::tool_selection::contracts::Evidence {
                start: 0,
                end: 6,
                text: "search".to_string(),
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
    let mut service = ToolSelectionService::new(
        writer.clone(),
        hash_embedding(),
        Arc::new(FixedReranker::new(&[])),
        Arc::new(UnconfiguredExtractor),
        Arc::new(FixtureBackend::new()),
        f64::NEG_INFINITY,
    );
    service.set_mcp_manager(manager);
    let outcome = service.apply_parsed(&context, &parsed).expect("apply");
    assert!(outcome.ambiguous);
    let rules: i64 = writer
        .read_serialized(|c| {
            c.query_row("SELECT COUNT(*) FROM tool_selection_rules", [], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(rules, 0);
}
// ---------------------------------------------------------------------------------------------
// A-series: scale and gateway shape
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a01_two_sources_with_1500_tools_publish_and_search_is_bounded() {
    let state_a = Arc::new(ServerState {
        protocol_version: Mutex::new(super::MCP_PROTOCOL_VERSION.to_string()),
        tools_capability: AtomicBool::new(true),
        ..ServerState::default()
    });
    let state_b = Arc::new(ServerState {
        protocol_version: Mutex::new(super::MCP_PROTOCOL_VERSION.to_string()),
        tools_capability: AtomicBool::new(true),
        ..ServerState::default()
    });
    let page = |prefix: &str, start: usize, count: usize| {
        let tools: Vec<Value> = (start..start + count)
            .map(|index| {
                json!({
                    "name": format!("{prefix}_{index:04}"),
                    "description": format!("Tool {index} for {prefix}."),
                    "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } }
                })
            })
            .collect();
        json!({ "tools": tools })
    };
    state_a.list_pages.lock().unwrap().clear();
    for chunk in 0..5 {
        state_a
            .list_pages
            .lock()
            .unwrap()
            .push(page("alpha", chunk * 100, 100));
    }
    state_b.list_pages.lock().unwrap().clear();
    for chunk in 0..10 {
        state_b
            .list_pages
            .lock()
            .unwrap()
            .push(page("beta", chunk * 100, 100));
    }
    let server_a = MockServer::start(state_a).await;
    let server_b = MockServer::start(state_b).await;

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
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    let principal = crate::tool_selection::service::ensure_principal(&writer).expect("principal");
    let grants: Vec<McpGrantSpec> = Vec::new();
    let sources = McpSources {
        sources: vec![
            McpSourceSpec {
                id: "alpha".to_string(),
                url: server_a.url(),
                enabled: true,
                bearer_token_env: None,
                grants: grants.clone(),
            },
            McpSourceSpec {
                id: "beta".to_string(),
                url: server_b.url(),
                enabled: true,
                bearer_token_env: None,
                grants,
            },
        ],
    };
    // Grant every tool through a wildcard-free loop after sync, matching how the config grants
    // are declarative per tool. For scale we insert user grants directly.
    let manager = McpManager::new(
        writer.clone(),
        principal.clone(),
        None,
        sources,
        Some(hash_embedding()),
        None,
    );
    manager.sync_source("alpha").await.expect("alpha");
    manager.sync_source("beta").await.expect("beta");
    let tool_count = writer
        .read_serialized(|c| mcp_repo::published_tool_count(c, "alpha").map_err(|e| e.to_string()))
        .unwrap();
    assert_eq!(tool_count, 500);
    writer
        .write({
            let principal = principal.clone();
            move |connection| {
                let tx = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .map_err(crate::database_error)?;
                tx.execute(
                    "INSERT OR IGNORE INTO tool_selection_grants(principal_id, tool_id, scope_kind, scope_id)
                     SELECT ?1, id, 'user', ?1 FROM tool_selection_catalog",
                    rusqlite::params![principal],
                )
                .map_err(|e| e.to_string())?;
                repository::bump_epochs(&tx, false, true, false).map_err(|e| e.to_string())?;
                tx.commit().map_err(crate::database_error)?;
                Ok(())
            }
        })
        .unwrap();

    let mut service = ToolSelectionService::new(
        writer.clone(),
        hash_embedding(),
        Arc::new(FixedReranker::new(&[])),
        Arc::new(UnconfiguredExtractor),
        Arc::new(FixtureBackend::new()),
        f64::NEG_INFINITY,
    );
    service.set_discovery_configured(true);
    service.set_mcp_manager(manager);
    let context =
        RequestContext::new(&principal, "conversation-d4").with_run(Some("run-d4".into()));
    service.set_scenario(&context, scenario());
    let response = service
        .search(&context, "alpha 0001", 8)
        .await
        .expect("search");
    assert!(response.candidates.len() <= 8);
    // The three conversation tools are still exactly three definitions.
    let definitions = crate::tool_selection::gateway::definitions();
    assert_eq!(definitions.len(), 3);
}
// Alias so the tests can call the MCP table accessors without a long path.
use super::repository as mcp_repo;
