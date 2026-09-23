use super::*;
#[tokio::test]
pub(super) async fn h10_large_mcp_result_pages_over_the_published_wire() {
    let state = default_state();
    let payload: String = "あ".repeat(40 * 1024);
    *state.call_result.lock().unwrap() = json!({
        "content": [{ "type": "text", "text": payload }],
        "isError": false,
    });
    let mock = MockServer::start(state).await;
    let harness = Harness::new(&mock, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");
    let Harness {
        writer, service, ..
    } = harness;
    let (config, directory) = d5_config();
    let server = crate::tool_selection::mcp_server::start(Arc::new(service), writer, config)
        .await
        .expect("start");
    let base = format!("http://127.0.0.1:{}/mcp", server.port());
    let token = d5_token(&directory);
    let client = reqwest::Client::new();

    // initialize + initialized
    let response = d5_post(
        &client,
        &base,
        &token,
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
        &client,
        &base,
        &token,
        Some(&session),
        &json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        }),
    )
    .await;

    async fn d5_tool_call(
        client: &reqwest::Client,
        base: &str,
        token: &str,
        session: &str,
        id: i64,
        name: &str,
        arguments: Value,
    ) -> reqwest::Response {
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
    }

    let search: Value = d5_tool_call(
        &client,
        &base,
        &token,
        &session,
        2,
        "tools_search",
        json!({ "intent": "search notes" }),
    )
    .await
    .json()
    .await
    .expect("search");
    let candidate = search
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str::<Value>(text).expect("envelope"))
        .and_then(|envelope| {
            envelope
                .pointer("/data/candidates/0/candidateRef")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .expect("candidate");

    let describe: Value = d5_tool_call(
        &client,
        &base,
        &token,
        &session,
        3,
        "tools_describe",
        json!({ "candidateRef": candidate }),
    )
    .await
    .json()
    .await
    .expect("describe");
    let execution_ref = describe
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str::<Value>(text).expect("envelope"))
        .and_then(|envelope| {
            envelope
                .pointer("/data/executionRef")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .expect("execution ref");

    let invoke: Value = d5_tool_call(
        &client,
        &base,
        &token,
        &session,
        4,
        "tools_invoke",
        json!({ "executionRef": execution_ref, "arguments": { "q": "value" } }),
    )
    .await
    .json()
    .await
    .expect("invoke");
    let invoked = invoke
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .map(|text| serde_json::from_str::<Value>(text).expect("envelope"))
        .expect("envelope");
    assert_eq!(invoked.pointer("/data/status"), Some(&json!("succeeded")));
    assert_eq!(
        invoked.pointer("/data/resultAvailability"),
        Some(&json!("stored"))
    );
    let result_ref = invoked
        .pointer("/data/resultRef")
        .and_then(Value::as_str)
        .expect("result ref")
        .to_string();
    let page_count = invoked
        .pointer("/data/pageCount")
        .and_then(Value::as_i64)
        .expect("page count");
    assert!(page_count > 1);

    let mut reassembled = String::new();
    for page in 0..page_count {
        let response = d5_tool_call(
            &client,
            &base,
            &token,
            &session,
            10 + page,
            "tools_describe",
            json!({ "resultRef": result_ref, "page": page }),
        )
        .await;
        // The whole wire response stays inside the 40 KiB bound.
        let bytes = response.bytes().await.expect("bytes");
        assert!(bytes.len() <= 40 * 1024, "wire page exceeded the bound");
        let value: Value = serde_json::from_slice(&bytes).expect("json");
        let text = value
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .map(|text| serde_json::from_str::<Value>(text).expect("envelope"))
            .and_then(|envelope| {
                envelope
                    .pointer("/data/text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .expect("page text");
        reassembled.push_str(&text);
    }
    let expected = descriptors::canonical_json_string(&json!({
        "content": [{ "type": "text", "text": payload }],
        "isError": false,
    }));
    assert_eq!(reassembled, expected);
    server.shutdown().await;
}
#[tokio::test]
pub(super) async fn review_self_endpoint_source_is_refused() {
    let state = default_state();
    let mock = MockServer::start(state).await;
    let harness = Harness::new(&mock, vec![user_grant("search")]);
    // The manager is told this process listens on 43127; a source pointing there is its own
    // gateway and must never be connected or synced.
    harness
        .manager
        .set_self_endpoint(Some("loopback:43127/mcp".to_string()))
        .await;
    for url in [
        "http://127.0.0.1:43127/mcp",
        "http://localhost:43127/mcp",
        "http://127.0.0.1:43127/mcp/",
        "http://[::1]:43127/mcp",
        "https://127.0.0.1:43127/mcp",
    ] {
        harness
            .manager
            .apply_sources(McpSources {
                sources: vec![McpSourceSpec {
                    id: "mcp-test".to_string(),
                    url: url.to_string(),
                    enabled: true,
                    bearer_token_env: None,
                    grants: vec![],
                }],
            })
            .await;
        let error = harness
            .manager
            .sync_source("mcp-test")
            .await
            .expect_err("self reference must be refused");
        assert_eq!(error.code, "source-self-reference", "for {url}");
    }
}
pub(super) async fn d5_execution_ref(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    session: &str,
    id: i64,
    candidate_ref: &str,
) -> String {
    let describe = d5_envelope(
        &d5_tool(
            client,
            base,
            token,
            session,
            id,
            "tools_describe",
            json!({ "candidateRef": candidate_ref }),
        )
        .await,
    );
    describe
        .pointer("/data/executionRef")
        .and_then(Value::as_str)
        .expect("execution ref")
        .to_string()
}
#[tokio::test]
pub(super) async fn h04_same_name_routes_to_the_selected_source_over_the_wire() {
    let state = default_state();
    let mock = MockServer::start(state.clone()).await;
    let harness = Harness::new(&mock, vec![user_grant("search")]);
    harness.manager.sync_source("mcp-test").await.expect("sync");

    // An L-Lang tool with the same display name as the external MCP tool.
    let principal = harness.principal.clone();
    let entry = crate::tool_selection::catalog::CatalogEntry {
        tool_id: "llang-search".to_string(),
        backend_key: "search".to_string(),
        title: "search".to_string(),
        purpose: "Search notes locally for decision records.".to_string(),
        operations: vec!["search".to_string()],
        objects: vec!["decision_record".to_string()],
        suitable: vec!["search notes".to_string()],
        unsuitable: vec!["unrelated chat".to_string()],
        required_inputs: vec!["q".to_string()],
        input_schema: json!({
            "type": "object",
            "properties": { "q": { "type": "string" } },
            "additionalProperties": false
        }),
        output_schema: None,
        effect: "read",
        usage_pages: vec![crate::tool_selection::catalog::UsagePage {
            section: "usage",
            page: 0,
            text: "Use search locally.".to_string(),
        }],
        backend_binding: json!({
            "capabilityId": "llang-search",
            "revisionId": "llang-search-rev1",
            "packageHash": "p",
            "contractHash": "c",
            "catalogEpoch": 0,
            "inputFields": []
        }),
    };
    harness
        .writer
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

    let Harness {
        writer, service, ..
    } = harness;
    let (config, directory) = d5_config();
    let server = crate::tool_selection::mcp_server::start(Arc::new(service), writer, config)
        .await
        .expect("start");
    let base = format!("http://127.0.0.1:{}/mcp", server.port());
    let token = d5_token(&directory);
    let client = reqwest::Client::new();
    let session = d5_open_session(&client, &base, &token).await;

    let search = d5_envelope(
        &d5_tool(
            &client,
            &base,
            &token,
            &session,
            2,
            "tools_search",
            json!({ "intent": "search notes", "limit": 8 }),
        )
        .await,
    );
    let candidates = search
        .pointer("/data/candidates")
        .and_then(Value::as_array)
        .expect("candidates");
    let find = |source: &str| {
        candidates
            .iter()
            .find(|candidate| candidate.get("sourceId").and_then(Value::as_str) == Some(source))
            .unwrap_or_else(|| panic!("missing candidate for {source}: {search}"))
            .clone()
    };
    let mcp_candidate = find("mcp-test");
    let llang_candidate = find("llang");

    // Invoking the MCP candidate reaches the external server exactly once.
    let before = state.call_count.load(Ordering::SeqCst);
    let mcp_ref = d5_execution_ref(
        &client,
        &base,
        &token,
        &session,
        3,
        mcp_candidate["candidateRef"]
            .as_str()
            .expect("candidateRef"),
    )
    .await;
    let mcp_result = d5_envelope(
        &d5_tool(
            &client,
            &base,
            &token,
            &session,
            4,
            "tools_invoke",
            json!({ "executionRef": mcp_ref, "arguments": { "q": "v" } }),
        )
        .await,
    );
    assert_eq!(
        mcp_result.pointer("/data/status"),
        Some(&json!("succeeded"))
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), before + 1);

    // Invoking the L-Lang candidate never contacts the external server.
    let llang_ref = d5_execution_ref(
        &client,
        &base,
        &token,
        &session,
        5,
        llang_candidate["candidateRef"]
            .as_str()
            .expect("candidateRef"),
    )
    .await;
    let llang_result = d5_envelope(
        &d5_tool(
            &client,
            &base,
            &token,
            &session,
            6,
            "tools_invoke",
            json!({ "executionRef": llang_ref, "arguments": { "q": "v" } }),
        )
        .await,
    );
    assert_eq!(
        llang_result.pointer("/data/status"),
        Some(&json!("succeeded"))
    );
    assert_eq!(state.call_count.load(Ordering::SeqCst), before + 1);

    server.shutdown().await;
}
