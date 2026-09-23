use super::*;
#[tokio::test]
pub(super) async fn h08_delete_cancels_the_call_and_closes_the_session() {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (writer, service) = ledger_with_backend(
        4,
        Arc::new(BlockingBackend {
            entered: entered.clone(),
            release,
        }),
    );
    let harness = serve_with(writer.clone(), service).await;
    let session = harness.ready_session().await;
    let execution_ref = prepare_execution_ref(&harness, &session).await;
    let call = spawn_tool_call(&harness, &session, 3, &execution_ref);
    let _entry = tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .expect("backend entered")
        .expect("permit");

    let response = harness
        .client
        .delete(&harness.base)
        .header("Authorization", format!("Bearer {}", harness.token))
        .header("Accept", ACCEPT_BOTH)
        .header("Mcp-Session-Id", &session)
        .send()
        .await
        .expect("delete");
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    let _ = tokio::time::timeout(Duration::from_secs(5), call).await;
    assert_eq!(wait_for_invocation_status(&writer).await, "cancelled");

    // The session no longer exists.
    let (status, _) = harness
        .rpc(
            &json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" }),
            Some(&session),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
}
#[tokio::test]
pub(super) async fn h08_shutdown_cancels_in_flight_calls() {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (writer, service) = ledger_with_backend(
        4,
        Arc::new(BlockingBackend {
            entered: entered.clone(),
            release,
        }),
    );
    let harness = serve_with(writer.clone(), service).await;
    let session = harness.ready_session().await;
    let execution_ref = prepare_execution_ref(&harness, &session).await;
    let call = spawn_tool_call(&harness, &session, 3, &execution_ref);
    let _entry = tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .expect("backend entered")
        .expect("permit");
    harness.server.shutdown().await;
    let _ = tokio::time::timeout(Duration::from_secs(5), call).await;
    assert_eq!(wait_for_invocation_status(&writer).await, "cancelled");
}
#[tokio::test]
pub(super) async fn h08_backend_panic_still_settles_the_row() {
    let (writer, service) = ledger_with_backend(4, Arc::new(PanicBackend));
    let harness = serve_with(writer.clone(), service).await;
    let session = harness.ready_session().await;
    let execution_ref = prepare_execution_ref(&harness, &session).await;
    let value = harness
        .call(
            3,
            "tools_invoke",
            json!({ "executionRef": execution_ref, "arguments": { "q": "v" } }),
            &session,
        )
        .await;
    assert_eq!(
        value.pointer("/result/isError"),
        Some(&json!(true)),
        "a panicking backend is reported as an error"
    );
    assert_eq!(wait_for_invocation_status(&writer).await, "interrupted");
}
// ---------------------------------------------------------------------------------------------
// S09: independent client smoke and latency measurement
// ---------------------------------------------------------------------------------------------

#[tokio::test]
pub(super) async fn s09_independent_client_smoke() {
    let harness = harness(4).await;
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/tool-selection/d5_mcp_smoke.py");
    let output = tokio::process::Command::new("python3")
        .arg(&script)
        .env("D5_MCP_URL", &harness.base)
        .env("D5_MCP_TOKEN", &harness.token)
        .output()
        .await
        .expect("spawn independent smoke client");
    assert!(
        output.status.success(),
        "smoke client failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("D5_SMOKE_OK"), "stdout: {stdout}");
}
#[tokio::test]
pub(super) async fn s09_search_latency_at_scale() {
    // 1,500 tools keeps this test light enough to run beside the timing-sensitive Codex
    // contract tests. The 10,000-item measurement is the live E5/BGE lane in P04 (§4.1), which is
    // a stronger measurement than the hash lane here.
    let count = 1500_usize;
    let iterations = 40_usize;
    let harness = harness(count).await;
    let session = harness.ready_session().await;
    let mut samples = Vec::with_capacity(iterations);
    for index in 0..iterations {
        let start = std::time::Instant::now();
        let value = harness
            .call(
                100 + index as i64,
                "tools_search",
                json!({ "intent": "search notes", "limit": 1 }),
                &session,
            )
            .await;
        assert_eq!(
            value.pointer("/result/isError"),
            Some(&json!(false)),
            "search failed at {count} tools: {value}"
        );
        samples.push(start.elapsed().as_millis() as u64);
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95) / 100];
    eprintln!("D5_PERF tools={count} n={iterations} p50={p50}ms p95={p95}ms");
    // A loose sanity bound only; the recorded p50/p95 are the measurement.
    assert!(p95 < 3_000, "p95 {p95}ms at {count} tools");
    harness.server.shutdown().await;
}
#[tokio::test]
pub(super) async fn review_deadline_keeps_the_call_slot_until_the_management_task_settles() {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (writer, service) = ledger_with_backend(
        4,
        Arc::new(StubbornBackend {
            entered: entered.clone(),
            release: release.clone(),
        }),
    );
    let harness = serve_with(writer.clone(), service).await;
    harness
        .server
        .inner
        .call_deadline_ms
        .store(150, std::sync::atomic::Ordering::SeqCst);
    let session = harness.ready_session().await;
    let execution_ref = prepare_execution_ref(&harness, &session).await;
    // The response times out, but this backend ignores cancellation and is still running.
    let value = harness
        .call(
            3,
            "tools_invoke",
            json!({ "executionRef": execution_ref, "arguments": { "q": "v" } }),
            &session,
        )
        .await;
    assert_eq!(value.pointer("/error/code"), Some(&json!(-32001)));
    let _entered = tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .expect("backend entered")
        .expect("permit");
    // The management task owns the slot: the handler returning on deadline must not release it.
    assert_eq!(harness.server.inner.sessions.global_in_flight(), 1);
    release.add_permits(1);
    for _ in 0..500 {
        if harness.server.inner.sessions.global_in_flight() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(harness.server.inner.sessions.global_in_flight(), 0);
    assert_eq!(wait_for_invocation_status(&writer).await, "succeeded");
    harness.server.shutdown().await;
}
#[tokio::test]
pub(super) async fn h08_deadline_cancels_and_settles_the_call() {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (writer, service) = ledger_with_backend(
        4,
        Arc::new(BlockingBackend {
            entered: entered.clone(),
            release,
        }),
    );
    let harness = serve_with(writer.clone(), service).await;
    // The production deadline is 30s; the test injects a short one to exercise the same path.
    harness
        .server
        .inner
        .call_deadline_ms
        .store(200, std::sync::atomic::Ordering::SeqCst);
    let session = harness.ready_session().await;
    let execution_ref = prepare_execution_ref(&harness, &session).await;
    let value = harness
        .call(
            3,
            "tools_invoke",
            json!({ "executionRef": execution_ref, "arguments": { "q": "v" } }),
            &session,
        )
        .await;
    assert_eq!(value.pointer("/error/code"), Some(&json!(-32001)));
    // The management task still reaches a terminal DB state after the deadline.
    assert_eq!(wait_for_invocation_status(&writer).await, "cancelled");
}
#[tokio::test]
pub(super) async fn review_delete_of_an_expired_session_is_not_found() {
    let harness = harness(1).await;
    let session = harness.ready_session().await;
    let arc = harness
        .server
        .inner
        .sessions
        .get(&session)
        .expect("session");
    arc.force_last_activity(now_ms() - super::super::sessions::SESSION_IDLE_TTL_MILLIS - 1);
    let response = harness
        .client
        .delete(&harness.base)
        .header("Authorization", format!("Bearer {}", harness.token))
        .header("Accept", ACCEPT_BOTH)
        .header("Mcp-Session-Id", &session)
        .send()
        .await
        .expect("delete");
    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    harness.server.shutdown().await;
}
#[tokio::test]
pub(super) async fn review_never_ready_session_reclaims_its_conversation() {
    let harness = harness(1).await;
    // initialize without notifications/initialized, then DELETE.
    let session = harness.initialize(1).await;
    let response = harness
        .client
        .delete(&harness.base)
        .header("Authorization", format!("Bearer {}", harness.token))
        .header("Accept", ACCEPT_BOTH)
        .header("Mcp-Session-Id", &session)
        .send()
        .await
        .expect("delete");
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    let conversations: i64 = harness
        .writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM conversations WHERE id LIKE 'mcpconv%'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("conversations");
    assert_eq!(
        conversations, 0,
        "an abandoned initialization must not leave a conversation row"
    );
}
