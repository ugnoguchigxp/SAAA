use super::*;
// ---------------------------------------------------------------------------------------------
// T02 migration
// ---------------------------------------------------------------------------------------------

#[test]
pub(super) fn t02_v23_sources_are_rebuilt_without_losing_data() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("ledger.sqlite");
    let connection = Connection::open(&path).expect("open");
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE tool_selection_sources (
               id TEXT PRIMARY KEY,
               kind TEXT NOT NULL CHECK(kind IN ('llang')),
               owner_principal TEXT NOT NULL CHECK(length(owner_principal) BETWEEN 1 AND 160),
               enabled INTEGER NOT NULL CHECK(enabled IN (0, 1))
             );
             CREATE TABLE tool_selection_catalog (
               id TEXT PRIMARY KEY,
               source_id TEXT NOT NULL,
               backend_key TEXT NOT NULL,
               current_revision_id TEXT,
               enabled INTEGER NOT NULL,
               FOREIGN KEY(source_id) REFERENCES tool_selection_sources(id)
             );
             INSERT INTO tool_selection_sources(id, kind, owner_principal, enabled)
               VALUES ('llang', 'llang', 'P1', 1);
             INSERT INTO tool_selection_catalog(id, source_id, backend_key, current_revision_id, enabled)
               VALUES ('legacy-tool', 'llang', 'web', NULL, 1);",
        )
        .expect("old schema");
    // Running the current migration widens the CHECK constraint and preserves rows.
    crate::persistence::schema::initialize_database(&connection).expect("migrate");
    let kind: String = connection
        .query_row(
            "SELECT kind FROM tool_selection_sources WHERE id = 'llang'",
            [],
            |row| row.get(0),
        )
        .expect("preserved source");
    assert_eq!(kind, "llang");
    let catalog: i64 = connection
        .query_row("SELECT COUNT(*) FROM tool_selection_catalog", [], |row| {
            row.get(0)
        })
        .expect("catalog");
    assert_eq!(catalog, 1);
    // The new kind is accepted by the rebuilt CHECK constraint.
    connection
        .execute(
            "INSERT INTO tool_selection_sources(id, kind, owner_principal, enabled)
             VALUES ('mcp', 'mcp_http', 'P1', 1)",
            [],
        )
        .expect("mcp_http accepted");
    let violations: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .expect("fk check");
    assert_eq!(violations, 0);
    drop(connection);
    // The database reopens with the same schema version.
    let reopened = Connection::open(&path).expect("reopen");
    let version: i64 = reopened
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version");
    assert_eq!(version, crate::persistence::schema::DATABASE_SCHEMA_VERSION);
}
// ---------------------------------------------------------------------------------------------
// T06 pending grants appear after a later sync
// ---------------------------------------------------------------------------------------------

#[tokio::test]
pub(super) async fn t06_grant_declared_before_the_tool_appears_is_applied_on_a_later_sync() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("later")]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("first");
    let later_id = descriptors::tool_id("mcp-test", "later");
    let principal = harness.principal.clone();
    let before = harness
        .writer
        .read_serialized({
            let principal = principal.clone();
            let later_id = later_id.clone();
            move |c| {
                repository::grant_exists(c, &principal, &later_id, None).map_err(|e| e.to_string())
            }
        })
        .unwrap();
    assert!(!before, "a declaration for a missing tool stays pending");

    state.list_pages.lock().unwrap()[0]["tools"] = json!([
        { "name": "later", "inputSchema": { "type": "object" } }
    ]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("second");
    let after = harness
        .writer
        .read_serialized(move |c| {
            repository::grant_exists(c, &principal, &later_id, None).map_err(|e| e.to_string())
        })
        .unwrap();
    assert!(
        after,
        "the pending declaration is applied once the name appears"
    );
}
// ---------------------------------------------------------------------------------------------
// T07 binding validation refuses before send
// ---------------------------------------------------------------------------------------------

#[tokio::test]
pub(super) async fn t07_endpoint_change_and_unknown_kind_are_refused_before_send() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let backend = McpBackend::new(harness.manager.clone());
    let cancellation = crate::RunCancellation::default();

    // A changed endpoint hash is rejected before any HTTP call.
    let tampered = crate::tool_selection::backends::BackendRequest {
        call_id: "c1".to_string(),
        tool_id: descriptors::tool_id("mcp-test", "search"),
        revision_id: "r1".to_string(),
        backend_key: "search".to_string(),
        binding: json!({
            "kind": "mcp_http",
            "sourceId": "mcp-test",
            "toolName": "search",
            "endpointHash": "f".repeat(64),
        }),
        arguments: json!({ "q": "v" }),
        timeout: std::time::Duration::from_secs(5),
        origin: "mcp",
        actor: None,
    };
    let outcome =
        crate::tool_selection::backends::ToolBackend::invoke(&backend, tampered, &cancellation)
            .await;
    assert_eq!(
        outcome.status,
        crate::tool_selection::backends::TechnicalStatus::Failed
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), 0);

    // A missing kind never falls back to the remote backend. A complete legacy L-Lang binding is
    // recognized; anything else is refused.
    let llang_binding = json!({
        "capabilityId": "x",
        "revisionId": "r",
        "packageHash": "p",
        "contractHash": "c",
        "catalogEpoch": 0,
        "inputFields": []
    });
    assert_eq!(BackendRouter::kind(&llang_binding), "llang");
    assert_eq!(
        BackendRouter::kind(&json!({ "capabilityId": "x" })),
        "unknown"
    );
    assert_eq!(BackendRouter::kind(&json!({ "kind": "other" })), "unknown");
}
// ---------------------------------------------------------------------------------------------
// T09 degraded embedding lane
// ---------------------------------------------------------------------------------------------

#[tokio::test]
pub(super) async fn t09_missing_embeddings_degrade_to_lexical_without_inventing_confidence() {
    let state = default_state();
    let server = MockServer::start(state).await;
    // A manager with no embedding provider leaves the vector lane empty.
    let harness = Harness::new_with_embedder(&server, vec![user_grant("search")], false);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let context = harness.context();
    harness.service.set_scenario(&context, scenario());
    let response = harness
        .service
        .search(&context, "search notes", 3)
        .await
        .expect("search");
    assert_eq!(
        response.status,
        crate::tool_selection::contracts::DecisionStatus::Degraded
    );
    assert!(response.degraded);
}
// ---------------------------------------------------------------------------------------------
// A06/A15 transport edge cases
// ---------------------------------------------------------------------------------------------

#[tokio::test]
pub(super) async fn a06_progress_notifications_are_ignored_and_the_result_is_returned() {
    let state = default_state();
    state.sse.store(true, Ordering::SeqCst);
    state.emit_progress.store(true, Ordering::SeqCst);
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "v" })).await;
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
}
#[tokio::test]
pub(super) async fn a15_unauthorized_is_a_remote_failure() {
    let state = default_state();
    state.unauthorized.store(true, Ordering::SeqCst);
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    let error = harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect_err("401 fails the sync");
    assert_eq!(error.code, "remote-unauthorized");
    // No source success is recorded for an unauthorized server.
    let fresh = harness
        .writer
        .read_serialized(|c| mcp_repo::is_fresh(c, "mcp-test", now_ms()).map_err(|e| e.to_string()))
        .unwrap();
    assert!(!fresh);
}
#[tokio::test]
pub(super) async fn a15_unsupported_server_request_is_refused_with_method_not_found() {
    let state = default_state();
    state.server_request.store(true, Ordering::SeqCst);
    state.get_supported.store(true, Ordering::SeqCst);
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    // The GET watcher reads the server request and replies with -32601; the request is never run.
    let mut reply = None;
    for _ in 0..100 {
        reply = *state.unsupported_reply.lock().unwrap();
        if reply.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(reply, Some(-32601));
}
#[tokio::test]
pub(super) async fn t07_restart_reconcile_marks_mcp_calls_as_indeterminate() {
    let state = default_state();
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let tool_id = descriptors::tool_id("mcp-test", "search");
    let revision_id = harness
        .writer
        .read_serialized(move |c| {
            repository::tool_by_id(c, &tool_id)
                .map_err(|e| e.to_string())?
                .and_then(|tool| tool.current_revision_id)
                .ok_or_else(|| "missing".to_string())
        })
        .unwrap();
    harness
        .writer
        .write({
            let revision_id = revision_id.clone();
            move |connection| {
                connection
                    .execute(
                        "INSERT INTO tool_selection_invocations(
                           id, decision_id, revision_id, technical_status, satisfaction, started_at)
                         VALUES ('mcp-running', NULL, ?1, 'running', 'unknown', 1)",
                        rusqlite::params![revision_id],
                    )
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    crate::tool_selection::service::reconcile_interrupted_invocations(&harness.writer)
        .expect("reconcile");
    let (status, error): (String, Option<String>) = harness
        .writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT technical_status, error_code FROM tool_selection_invocations WHERE id = 'mcp-running'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(status, "interrupted");
    assert_eq!(error.as_deref(), Some("remote-outcome-unknown"));
}
// ---------------------------------------------------------------------------------------------
// Review fixes: resolution, backoff, admission, removal race, manual grants, project ACL
// ---------------------------------------------------------------------------------------------

#[test]
pub(super) fn review_source_qualified_resolution_is_unambiguous() {
    let connection = Connection::open_in_memory().expect("in-memory");
    crate::persistence::schema::initialize_database(&connection).expect("schema");
    let principal = "P-REVIEW";
    let a = descriptors::tool_id("mcp-a", "search");
    let b = descriptors::tool_id("mcp-b", "search");
    for (source, tool) in [("mcp-a", &a), ("mcp-b", &b)] {
        repository::upsert_source(&connection, source, "mcp_http", principal, true)
            .expect("source");
        repository::upsert_tool(
            &connection,
            &repository::NewTool {
                source_id: source,
                tool_id: tool,
                backend_key: "search",
                enabled: true,
            },
        )
        .expect("tool");
        repository::upsert_grant(&connection, principal, tool, "user", principal).expect("grant");
    }
    let context = RequestContext::new(principal, "conversation-d4").with_run(Some("run".into()));
    // A bare duplicate name is ambiguous.
    assert_eq!(
        crate::tool_selection::resolve::resolve_tool_id(&connection, "search", &context, None),
        None
    );
    // A source-qualified name resolves to exactly one tool.
    assert_eq!(
        crate::tool_selection::resolve::resolve_tool_id(
            &connection,
            "mcp-a/search",
            &context,
            None
        ),
        Some(a)
    );
    assert_eq!(
        crate::tool_selection::resolve::resolve_tool_id(
            &connection,
            "mcp-b/search",
            &context,
            None
        ),
        Some(b)
    );
}
#[tokio::test]
pub(super) async fn review_failed_initialize_sets_a_source_backoff() {
    let state = default_state();
    state.tools_capability.store(false, Ordering::SeqCst);
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    let first = harness.manager.sync_source("mcp-test").await.unwrap_err();
    assert_eq!(first.code, "tools-capability-missing");
    // The immediate retry is refused by the 1s backoff rather than hammering the server.
    let second = harness.manager.sync_source("mcp-test").await.unwrap_err();
    assert_eq!(second.code, "source-backoff");
}
#[tokio::test]
pub(super) async fn review_removal_during_first_sync_cannot_republish_the_source() {
    let state = default_state();
    state.gate_list.store(true, Ordering::SeqCst);
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    let manager = harness.manager.clone();
    let sync = tokio::spawn(async move { manager.sync_source("mcp-test").await });
    // Barrier: the server has received tools/list but has not answered it yet.
    state.list_received.notified().await;
    harness
        .manager
        .apply_sources(McpSources {
            sources: Vec::new(),
        })
        .await;
    state.list_release.notify_one();
    let outcome = sync.await.expect("join");
    assert_eq!(outcome.unwrap_err().code, "sync-generation-changed");
    let enabled = harness
        .writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT enabled FROM tool_selection_sources WHERE id = 'mcp-test'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(enabled, 0, "a removed source stays disabled");
    let tools = harness
        .writer
        .read_serialized(|c| {
            mcp_repo::published_tool_count(c, "mcp-test").map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(tools, 0, "no tool was published by the aborted sync");
}
#[tokio::test]
pub(super) async fn review_manual_grant_is_never_adopted_or_revoked() {
    let state = default_state();
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, Vec::new());
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let tool_id = descriptors::tool_id("mcp-test", "search");
    let principal = harness.principal.clone();
    harness
        .writer
        .write({
            let tool_id = tool_id.clone();
            let principal = principal.clone();
            move |c| {
                repository::upsert_grant(c, &principal, &tool_id, "user", &principal)
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    // Now the config declares the same grant and a sync runs.
    harness
        .manager
        .apply_sources(McpSources {
            sources: vec![McpSourceSpec {
                id: "mcp-test".to_string(),
                url: server.url(),
                enabled: true,
                bearer_token_env: None,
                grants: vec![user_grant("search")],
            }],
        })
        .await;
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("sync2");
    let managed = harness
        .writer
        .read_serialized(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM tool_selection_mcp_managed_grants",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(managed, 0, "a pre-existing manual grant is not adopted");
    // Removing the source must leave the manual grant intact.
    harness
        .manager
        .apply_sources(McpSources {
            sources: Vec::new(),
        })
        .await;
    let still_granted = harness
        .writer
        .read_serialized({
            let tool_id = tool_id.clone();
            let principal = principal.clone();
            move |c| {
                crate::tool_selection::source_lookup::exact_grant_exists(
                    c, &principal, &tool_id, "user", &principal,
                )
                .map_err(|e| e.to_string())
            }
        })
        .unwrap();
    assert!(still_granted, "the manual grant survives source removal");
}
