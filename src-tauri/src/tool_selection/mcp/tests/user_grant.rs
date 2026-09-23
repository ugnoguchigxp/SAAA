use super::*;
pub(crate) fn user_grant(tool_name: &str) -> McpGrantSpec {
    McpGrantSpec {
        tool_name: tool_name.to_string(),
        scope_kind: McpGrantScope::User,
        project_id: None,
    }
}
pub(crate) fn scenario() -> Scenario {
    Scenario {
        intent: "search notes".to_string(),
        operation: crate::tool_selection::Operation::Search,
        object_type: crate::tool_selection::ObjectType::Document,
        phase: crate::tool_selection::Phase::Discover,
        input_kind: crate::tool_selection::InputKind::Text,
    }
}
// ---------------------------------------------------------------------------------------------
// T01 configuration
// ---------------------------------------------------------------------------------------------

#[test]
pub(super) fn t01_config_rejects_bad_documents() {
    let cases: Vec<(&str, &[u8], Option<&str>)> = vec![
        ("ok", br#"{"formatVersion":1,"sources":[]}"#, None),
        ("unknown field", br#"{"formatVersion":1,"sources":[],"extra":1}"#, Some("mcp-sources-invalid-json")),
        ("bad version", br#"{"formatVersion":2,"sources":[]}"#, Some("mcp-sources-version-unknown")),
        ("bad id", br#"{"formatVersion":1,"sources":[{"id":"Bad","url":"https://x/mcp"}]}"#, Some("mcp-sources-id-invalid")),
        (
            "duplicate id",
            br#"{"formatVersion":1,"sources":[{"id":"a","url":"https://x/mcp"},{"id":"a","url":"https://y/mcp"}]}"#,
            Some("mcp-sources-duplicate-id"),
        ),
        ("insecure http", br#"{"formatVersion":1,"sources":[{"id":"a","url":"http://example.com/mcp"}]}"#, Some("mcp-sources-url-insecure")),
        ("userinfo", br#"{"formatVersion":1,"sources":[{"id":"a","url":"https://u:p@x/mcp"}]}"#, Some("mcp-sources-url-invalid")),
        ("fragment", br#"{"formatVersion":1,"sources":[{"id":"a","url":"https://x/mcp#f"}]}"#, Some("mcp-sources-url-invalid")),
        (
            "project grant without id",
            br#"{"formatVersion":1,"sources":[{"id":"a","url":"https://x/mcp","grants":[{"toolName":"t","scopeKind":"project"}]}]}"#,
            Some("mcp-sources-grant-invalid"),
        ),
    ];
    for (label, bytes, expected) in cases {
        let result = McpSources::parse(bytes);
        assert_eq!(result.as_ref().err().copied(), expected, "case {label}");
        if expected.is_none() {
            assert!(result.is_ok(), "case {label}");
        }
    }
}
#[test]
pub(super) fn t01_loopback_http_is_allowed_and_endpoint_hash_is_stable() {
    let parsed = McpSources::parse(
        br#"{"formatVersion":1,"sources":[{"id":"a","url":"http://127.0.0.1:8787/mcp","bearerTokenEnv":"SAAA_TEST_TOKEN"}]}"#,
    )
    .expect("valid");
    let source = &parsed.sources[0];
    assert_eq!(source.bearer_token_env.as_deref(), Some("SAAA_TEST_TOKEN"));
    // The endpoint hash is stable and drops the query so a token in a query parameter cannot be
    // embedded in a revision binding.
    let with_query = super::super::config::endpoint_hash("http://127.0.0.1:8787/mcp?token=secret");
    let without_query = super::super::config::endpoint_hash("http://127.0.0.1:8787/mcp");
    assert_eq!(with_query, without_query);
    assert_eq!(source.endpoint_hash().len(), 64);
}
#[test]
pub(super) fn t01_missing_token_environment_reports_unavailable() {
    let source = McpSourceSpec {
        id: "a".to_string(),
        url: "https://example.com/mcp".to_string(),
        enabled: true,
        bearer_token_env: Some("SAAA_D4_MISSING_TOKEN_FOR_TEST".to_string()),
        grants: Vec::new(),
    };
    assert!(source.resolve_token().is_none());
}
// ---------------------------------------------------------------------------------------------
// T03 descriptors and stable ids
// ---------------------------------------------------------------------------------------------

pub(super) fn descriptor(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } }, "required": ["q"] }
    })
}
#[test]
pub(super) fn t03_ids_are_stable_and_source_scoped() {
    let endpoint = "e".repeat(64);
    let a =
        descriptors::normalize_tool("src-a", &endpoint, &descriptor("search", "one")).expect("a");
    let b =
        descriptors::normalize_tool("src-b", &endpoint, &descriptor("search", "one")).expect("b");
    assert_ne!(
        a.tool_id, b.tool_id,
        "same name in different sources is a different tool"
    );
    assert!(a.tool_id.starts_with("mcpt_"));
    assert!(a.revision_id.starts_with("mcpr_"));

    // Key order changes do not move the revision id.
    let reordered = json!({
        "description": "one",
        "name": "search",
        "inputSchema": { "required": ["q"], "type": "object", "properties": { "q": { "type": "string" } } }
    });
    let c = descriptors::normalize_tool("src-a", &endpoint, &reordered).expect("c");
    assert_eq!(a.revision_id, c.revision_id);
    assert_eq!(a.tool_id, c.tool_id);
}
#[test]
pub(super) fn t03_a_to_b_to_a_returns_to_a_and_detects_changes() {
    let endpoint = "e".repeat(64);
    let a = descriptors::normalize_tool("src", &endpoint, &descriptor("x", "A")).expect("a");
    let b = descriptors::normalize_tool("src", &endpoint, &descriptor("x", "B")).expect("b");
    let a_again = descriptors::normalize_tool("src", &endpoint, &descriptor("x", "A")).expect("a2");
    assert_ne!(a.revision_id, b.revision_id);
    assert_eq!(a.revision_id, a_again.revision_id);
    assert_eq!(a.tool_id, b.tool_id);

    let other_endpoint =
        descriptors::normalize_tool("src", &"f".repeat(64), &descriptor("x", "A")).expect("e");
    assert_ne!(a.revision_id, other_endpoint.revision_id);
}
#[test]
pub(super) fn t03_unsupported_schema_fails_instead_of_substituting_empty() {
    let endpoint = "e".repeat(64);
    // A non-object schema is rejected; the sync must not substitute `{}`.
    let bad = json!({ "name": "x", "inputSchema": 5 });
    assert!(descriptors::normalize_tool("src", &endpoint, &bad).is_err());
}
#[test]
pub(super) fn t03_descriptor_hash_covers_annotations_and_management() {
    let endpoint = "e".repeat(64);
    let base = descriptor("x", "A");
    let annotated = json!({
        "name": "x",
        "description": "A",
        "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } }, "required": ["q"] },
        "annotations": { "readOnlyHint": true },
        "_meta": { "saaa": { "effect": "read", "operations": ["read"] } }
    });
    let a = descriptors::normalize_tool("src", &endpoint, &base).expect("a");
    let b = descriptors::normalize_tool("src", &endpoint, &annotated).expect("b");
    assert_ne!(a.revision_id, b.revision_id);
    assert_eq!(b.entry.effect, "read");
    assert_eq!(b.entry.operations, vec!["read".to_string()]);
}
// ---------------------------------------------------------------------------------------------
// T04 transport and session
// ---------------------------------------------------------------------------------------------

#[tokio::test]
pub(super) async fn t04_json_and_sse_tools_call_both_work() {
    for sse in [false, true] {
        let state = default_state();
        state.sse.store(sse, Ordering::SeqCst);
        let server = MockServer::start(state.clone()).await;
        let harness = Harness::new(&server, vec![user_grant("search")]);
        harness.manager.sync_source("mcp-test").await.expect("sync");
        assert!(state.initialized_seen.load(Ordering::SeqCst));
        let tools = harness
            .writer
            .read_serialized(|c| {
                mcp_repo::published_tool_count(c, "mcp-test").map_err(|e| e.to_string())
            })
            .unwrap();
        assert_eq!(tools, 2);
    }
}
#[tokio::test]
pub(super) async fn t04_session_404_triggers_reinitialize_for_the_next_operation() {
    let state = default_state();
    *state.session.lock().unwrap() = Some("sess-1".to_string());
    let server = MockServer::start(state.clone()).await;
    // First sync negotiates the session and succeeds.
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    // The server forgets the session. The operation that observes the 404 fails; the next one
    // re-initializes and succeeds. An in-flight operation is never replayed.
    *state.session.lock().unwrap() = Some("sess-2".to_string());
    assert!(harness.manager.sync_source("mcp-test").await.is_err());
    assert!(harness.manager.sync_source("mcp-test").await.is_ok());
}
#[tokio::test]
pub(super) async fn t04_get_405_is_not_an_error() {
    let state = default_state();
    state.get_supported.store(false, Ordering::SeqCst);
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    assert!(harness.manager.sync_source("mcp-test").await.is_ok());
}
// ---------------------------------------------------------------------------------------------
// T05 sync
// ---------------------------------------------------------------------------------------------

#[tokio::test]
pub(super) async fn t05_failed_page_keeps_the_previous_snapshot_and_epoch() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("first sync");
    let epoch_before = harness.epochs().catalog;
    let count_before = harness.count("tool_selection_catalog");

    // Page index 1 (the second tools/list call) is invalid JSON.
    *state.injected_bad_page.lock().unwrap() = Some(1);
    assert!(harness.manager.sync_source("mcp-test").await.is_err());
    assert_eq!(harness.epochs().catalog, epoch_before);
    assert_eq!(harness.count("tool_selection_catalog"), count_before);
}
#[tokio::test]
pub(super) async fn t05_same_content_resync_does_not_move_the_epoch() {
    let state = default_state();
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("first");
    let epoch = harness.epochs().catalog;
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("second");
    assert_eq!(harness.epochs().catalog, epoch);
}
#[tokio::test]
pub(super) async fn t05_changed_description_bumps_the_epoch_once() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("first");
    let epoch = harness.epochs().catalog;
    state.list_pages.lock().unwrap()[0]["tools"][0]["description"] =
        json!("Search notes, now with more detail.");
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("second");
    assert_eq!(harness.epochs().catalog, epoch + 1);
}
#[tokio::test]
pub(super) async fn t05_disappearing_tool_is_disabled_and_reappears() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("first");
    state.list_pages.lock().unwrap()[0]["tools"] = json!([{
        "name": "search",
        "description": "Search notes.",
        "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } }, "required": ["q"] }
    }]);
    harness
        .manager
        .sync_source("mcp-test")
        .await
        .expect("second");
    let other_id = descriptors::tool_id("mcp-test", "other");
    let enabled = harness
        .writer
        .read_serialized({
            let other_id = other_id.clone();
            move |c| mcp_repo::tool_enabled(c, &other_id).map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(enabled, Some(false));

    // The revision history is retained.
    let revisions: i64 = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM tool_selection_revisions WHERE tool_id = ?1",
                    rusqlite::params![other_id],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .unwrap();
    assert_eq!(revisions, 1);
}
#[tokio::test]
pub(super) async fn t05_cursor_loop_and_duplicate_names_fail_the_sync() {
    // Cursor loop: page 0 advertises cursor A, page 1 advertises cursor A again.
    let state = default_state();
    state.list_pages.lock().unwrap().clear();
    state.list_pages.lock().unwrap().push(json!({
        "tools": [{ "name": "a", "inputSchema": { "type": "object" } }],
        "nextCursor": "A"
    }));
    state.list_pages.lock().unwrap().push(json!({
        "tools": [{ "name": "b", "inputSchema": { "type": "object" } }],
        "nextCursor": "A"
    }));
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("a")]);
    assert_eq!(
        harness
            .manager
            .sync_source("mcp-test")
            .await
            .unwrap_err()
            .code,
        "sync-cursor-loop"
    );

    // Duplicate names in one snapshot.
    let state = default_state();
    state.list_pages.lock().unwrap()[0]["tools"] = json!([
        { "name": "dup", "inputSchema": { "type": "object" } },
        { "name": "dup", "inputSchema": { "type": "object" } }
    ]);
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("dup")]);
    assert_eq!(
        harness
            .manager
            .sync_source("mcp-test")
            .await
            .unwrap_err()
            .code,
        "sync-duplicate-name"
    );
}
#[tokio::test]
pub(super) async fn t05_empty_page_with_cursor_is_allowed() {
    let state = default_state();
    state.list_pages.lock().unwrap().clear();
    state
        .list_pages
        .lock()
        .unwrap()
        .push(json!({ "tools": [], "nextCursor": "B" }));
    state.list_pages.lock().unwrap().push(json!({
        "tools": [{ "name": "a", "inputSchema": { "type": "object" } }]
    }));
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("a")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    assert_eq!(harness.count("tool_selection_catalog"), 1);
}
// ---------------------------------------------------------------------------------------------
// T06 manager: grants and removal
// ---------------------------------------------------------------------------------------------

#[tokio::test]
pub(super) async fn t06_import_alone_creates_no_grant_and_config_grants_are_managed() {
    let state = default_state();
    let server = MockServer::start(state).await;
    // No grants declared: the tools are imported but not authorized.
    let harness = Harness::new(&server, Vec::new());
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let tool_id = descriptors::tool_id("mcp-test", "search");
    let principal = harness.principal.clone();
    let authorized = harness
        .writer
        .read_serialized({
            let principal = principal.clone();
            let tool_id = tool_id.clone();
            move |c| {
                repository::grant_exists(c, &principal, &tool_id, None).map_err(|e| e.to_string())
            }
        })
        .unwrap();
    assert!(!authorized, "import is never a grant");
    let eligibility = harness
        .writer
        .read_serialized(move |c| {
            repository::eligible_revisions(c, &principal, None, now_ms())
                .map(|rows| rows.len())
                .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(eligibility, 0);
}
#[tokio::test]
pub(super) async fn t06_removing_a_source_revokes_only_managed_grants() {
    let state = default_state();
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let search_id = descriptors::tool_id("mcp-test", "search");
    let other_id = descriptors::tool_id("mcp-test", "other");
    let principal = harness.principal.clone();
    // A manual grant outside the MCP config.
    harness
        .writer
        .write({
            let other_id = other_id.clone();
            let principal = principal.clone();
            move |connection| {
                repository::upsert_grant(connection, &principal, &other_id, "user", &principal)
                    .map_err(|e| e.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    let acl_before = harness.epochs().acl;

    harness
        .manager
        .apply_sources(McpSources {
            sources: Vec::new(),
        })
        .await;

    let (search_granted, other_granted) = harness
        .writer
        .read_serialized({
            let principal = principal.clone();
            move |c| {
                let search = repository::grant_exists(c, &principal, &search_id, None)
                    .map_err(|e| e.to_string())?;
                let other = repository::grant_exists(c, &principal, &other_id, None)
                    .map_err(|e| e.to_string())?;
                Ok((search, other))
            }
        })
        .unwrap();
    assert!(!search_granted, "config grant is revoked");
    assert!(other_granted, "manual grant is preserved");
    assert!(harness.epochs().acl > acl_before, "acl epoch moves");
    assert!(
        harness.epochs().catalog > 0,
        "catalog epoch moves when tools are disabled"
    );
}
// ---------------------------------------------------------------------------------------------
// T07/T08 invocation and continuation
// ---------------------------------------------------------------------------------------------

pub(crate) async fn describe_and_invoke(
    harness: &Harness,
    arguments: Value,
) -> crate::tool_selection::service::InvokeResponse {
    let context = harness.context();
    harness.service.set_scenario(&context, scenario());
    let search = harness
        .service
        .search(&context, "search notes", 1)
        .await
        .expect("search");
    assert!(!search.candidates.is_empty(), "candidate expected");
    let describe = harness
        .service
        .describe(&context, &search.candidates[0].reference, "contract", None)
        .expect("describe");
    let execution_ref = describe.execution_ref.expect("execution ref");
    let cancellation = crate::RunCancellation::default();
    harness
        .service
        .invoke(&context, &execution_ref, &arguments, &cancellation)
        .await
        .expect("invoke")
}
#[tokio::test]
pub(super) async fn t07_real_http_invoke_returns_success_and_records_one_call() {
    let state = default_state();
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "value" })).await;
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), 1);
}
#[tokio::test]
pub(super) async fn t07_is_error_result_is_failed_and_bounded() {
    let state = default_state();
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": "boom" }],
        "isError": true
    });
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "value" })).await;
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Failed
    );
    assert_eq!(response.error_code, Some("remote-tool-error"));
    assert!(response.result.is_some());
}
#[tokio::test]
pub(super) async fn t07_disconnect_after_side_effect_is_unknown_and_not_retried() {
    let state = default_state();
    state.disconnect_after_call.store(true, Ordering::SeqCst);
    let server = MockServer::start(state.clone()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let response = describe_and_invoke(&harness, json!({ "q": "value" })).await;
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Unknown
    );
    // The server observed exactly one side effect and the client never retried.
    assert_eq!(state.call_count.load(Ordering::SeqCst), 1);
}
