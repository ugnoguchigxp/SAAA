use super::*;
#[tokio::test]
pub(super) async fn review_after_send_cancel_is_unknown_and_releases_permits() {
    let state = default_state();
    state.gate_call.store(true, Ordering::SeqCst);
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
    let arguments = json!({ "q": "v" });
    let invoke = harness
        .service
        .invoke(&context, &execution_ref, &arguments, &cancellation);
    let controller = async {
        state.call_received.notified().await;
        cancellation.cancel();
        state.gate_call.store(false, Ordering::SeqCst);
        state.call_release.notify_one();
    };
    let (result, _) = tokio::join!(invoke, controller);
    let response = result.expect("invoke");
    assert_eq!(
        response.status,
        crate::tool_selection::backends::TechnicalStatus::Unknown
    );
    // The permit was released, so a second call is admitted and succeeds.
    let second = harness
        .service
        .invoke(
            &context,
            &execution_ref,
            &json!({ "q": "v" }),
            &crate::RunCancellation::default(),
        )
        .await
        .expect("second invoke");
    assert_eq!(
        second.status,
        crate::tool_selection::backends::TechnicalStatus::Succeeded
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), 2);
}
#[tokio::test]
pub(super) async fn review_project_scoped_result_is_refused_for_another_project() {
    let state = default_state();
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": "x".repeat(40 * 1024) }],
    });
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
                repository::upsert_grant(c, &principal, &tool_id, "project", "PROJ-1")
                    .map_err(|e| e.to_string())?;
                repository::bump_epochs(c, false, true, false).map_err(|e| e.to_string())?;
                Ok(())
            }
        })
        .unwrap();
    let context = RequestContext::new(&principal, "conversation-d4")
        .with_run(Some("run-proj".into()))
        .with_project(Some("PROJ-1".into()));
    harness.service.set_scenario(&context, scenario());
    let search = harness
        .service
        .search(&context, "search notes", 1)
        .await
        .expect("search");
    assert_eq!(search.candidates.len(), 1);
    let describe = harness
        .service
        .describe(&context, &search.candidates[0].reference, "contract", None)
        .expect("describe");
    let execution_ref = describe.execution_ref.expect("execution ref");
    let response = harness
        .service
        .invoke(
            &context,
            &execution_ref,
            &json!({ "q": "v" }),
            &crate::RunCancellation::default(),
        )
        .await
        .expect("invoke");
    let result_ref = response.result_ref.expect("result ref");
    assert!(harness
        .service
        .describe_result(&context, &result_ref, 0)
        .is_ok());
    // The same run scope but a different project must not read the stored result.
    let other = RequestContext::new(&principal, "conversation-d4")
        .with_run(Some("run-proj".into()))
        .with_project(Some("PROJ-2".into()));
    assert!(harness
        .service
        .describe_result(&other, &result_ref, 0)
        .is_err());
}
#[tokio::test]
pub(super) async fn review_describe_refuses_a_stale_source_reference() {
    let state = default_state();
    let server = MockServer::start(state).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let context = harness.context();
    harness.service.set_scenario(&context, scenario());
    let search = harness
        .service
        .search(&context, "search notes", 1)
        .await
        .expect("search");
    let candidate_ref = search.candidates[0].reference.clone();
    // Age the source beyond the freshness window after the reference was issued.
    harness
        .writer
        .write(|c| {
            c.execute(
                "UPDATE tool_selection_mcp_sources SET last_success_at = ?1 WHERE source_id = 'mcp-test'",
                rusqlite::params![now_ms() - super::MCP_SOURCE_STALE_AFTER_MILLIS - 1000],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    let error = harness
        .service
        .describe(&context, &candidate_ref, "contract", None)
        .unwrap_err();
    assert_eq!(
        error.code,
        crate::tool_selection::ToolSelectionErrorCode::StaleReference
    );
}
// ---------------------------------------------------------------------------------------------
// P02: correction rules are bound to the endpoint they were learned on
// ---------------------------------------------------------------------------------------------

/// Applies one avoid-correction for the remote tool `search` and returns the created rule count.
pub(super) fn apply_remote_avoid_correction(harness: &Harness) {
    harness
        .writer
        .write(|connection| {
            connection
                .execute(
                    "INSERT INTO conversation_messages(id, conversation_id, role, content, created_at)
                     VALUES ('msg-p02', 'conversation-d4', 'user', 'fixture', '1')",
                    [],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("message");
    let context = harness.context().with_message(Some("msg-p02".to_string()));
    let parsed = crate::tool_selection::feedback::ParsedExtraction {
        scenario: scenario(),
        accepted: vec![crate::tool_selection::contracts::ExtractedFeedback {
            kind: crate::tool_selection::contracts::FeedbackKind::ToolChoice,
            decision_id: None,
            rejected_tool_id: Some("search".to_string()),
            preferred_tool_id: None,
            scope: crate::tool_selection::contracts::ScopeKind::Conversation,
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
    let outcome = harness
        .service
        .apply_parsed(&context, &parsed)
        .expect("apply");
    assert!(outcome.applied, "the correction must become a rule");
}
pub(super) fn active_rule_ids(harness: &Harness) -> Vec<String> {
    harness
        .writer
        .read_serialized(|connection| {
            repository::active_rules(
                connection,
                &harness.principal,
                "conversation-d4",
                None,
                None,
                now_ms(),
            )
            .map(|rules| rules.into_iter().map(|rule| rule.id).collect())
            .map_err(|error| error.to_string())
        })
        .expect("active rules")
}
pub(super) fn binding_hash(harness: &Harness) -> Option<String> {
    harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT endpoint_hash FROM tool_selection_rule_source_bindings LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .ok()
}
#[tokio::test]
pub(super) async fn p02_remote_rule_is_bound_and_survives_only_the_learned_endpoint() {
    let server_a = MockServer::start(default_state()).await;
    let harness = Harness::new(&server_a, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    apply_remote_avoid_correction(&harness);

    let source_hash: String = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT endpoint_hash FROM tool_selection_mcp_sources WHERE source_id = 'mcp-test'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("source hash");
    assert_eq!(
        binding_hash(&harness).as_deref(),
        Some(source_hash.as_str()),
        "a rule learned on a remote tool records that endpoint"
    );
    assert_eq!(active_rule_ids(&harness).len(), 1);
    let epoch_before = harness.epochs().rule;

    // The source moves to a different endpoint: the learned correction must stop applying.
    let server_b = MockServer::start(default_state()).await;
    let moved = McpSources {
        sources: vec![McpSourceSpec {
            id: "mcp-test".to_string(),
            url: server_b.url(),
            enabled: true,
            bearer_token_env: None,
            grants: vec![user_grant("search")],
        }],
    };
    harness.manager.apply_sources(moved).await;
    assert_eq!(
        binding_hash(&harness).as_deref(),
        Some(""),
        "an endpoint change leaves the binding unconfirmed"
    );
    assert!(harness.epochs().rule > epoch_before);
    assert!(active_rule_ids(&harness).is_empty());

    // Returning to the original URL must not silently revive the stale correction.
    let restored = McpSources {
        sources: vec![McpSourceSpec {
            id: "mcp-test".to_string(),
            url: server_a.url(),
            enabled: true,
            bearer_token_env: None,
            grants: vec![user_grant("search")],
        }],
    };
    harness.manager.apply_sources(restored).await;
    assert_eq!(binding_hash(&harness).as_deref(), Some(""));
    assert!(active_rule_ids(&harness).is_empty());
}
#[tokio::test]
pub(super) async fn p02_backfill_marks_pre_version_remote_rules_unconfirmed() {
    let server = MockServer::start(default_state()).await;
    let harness = Harness::new(&server, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    apply_remote_avoid_correction(&harness);
    // Simulate a rule created before version 25: no binding row exists.
    harness
        .writer
        .write(|connection| {
            connection
                .execute("DELETE FROM tool_selection_rule_source_bindings", [])
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("delete binding");
    assert_eq!(active_rule_ids(&harness).len(), 1);

    harness
        .writer
        .write(|connection| {
            super::super::schema::backfill_rule_source_bindings(connection)
                .map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("backfill");
    assert_eq!(binding_hash(&harness).as_deref(), Some(""));
    assert!(active_rule_ids(&harness).is_empty());
}
// ---------------------------------------------------------------------------------------------
// D5: published server reusing the D4 ledger
// ---------------------------------------------------------------------------------------------

/// A D5 config with an owner-only token file; the tempdir must outlive the server.
pub(crate) fn d5_config() -> (
    crate::tool_selection::mcp_server::config::McpServerConfig,
    tempfile::TempDir,
) {
    use base64::Engine;
    let directory = tempfile::tempdir().expect("tempdir");
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([9u8; 32]);
    let path = directory.path().join("token");
    std::fs::write(&path, token).expect("write token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("mode");
    }
    (
        crate::tool_selection::mcp_server::config::McpServerConfig {
            enabled: true,
            port: 0,
            token_file: Some(path),
            project_id: None,
        },
        directory,
    )
}
pub(crate) fn d5_token(directory: &tempfile::TempDir) -> String {
    std::fs::read_to_string(directory.path().join("token"))
        .expect("token")
        .trim()
        .to_string()
}
pub(crate) async fn d5_post(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    session: Option<&str>,
    body: &Value,
) -> reqwest::Response {
    let mut request = client
        .post(base)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json, text/event-stream")
        .json(body);
    if let Some(session) = session {
        request = request.header("Mcp-Session-Id", session);
    }
    request.send().await.expect("request")
}
pub(crate) async fn d5_open_session(client: &reqwest::Client, base: &str, token: &str) -> String {
    let response = d5_post(
        client,
        base,
        token,
        None,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-06-18", "capabilities": {} }
        }),
    )
    .await;
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .expect("session")
        .to_string();
    d5_post(
        client,
        base,
        token,
        Some(&session),
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    session
}
pub(crate) async fn d5_tool(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    session: &str,
    id: i64,
    name: &str,
    arguments: Value,
) -> Value {
    d5_post(
        client,
        base,
        token,
        Some(session),
        &json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }),
    )
    .await
    .json::<Value>()
    .await
    .expect("json-rpc response")
}
pub(crate) fn d5_envelope(value: &Value) -> Value {
    let text = value
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .expect("content text");
    serde_json::from_str(text).expect("envelope")
}
