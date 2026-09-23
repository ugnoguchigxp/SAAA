use super::*;
#[tokio::test]
pub(super) async fn rr_11_detached_owner_settles() {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (writer, service) = ledger_with_backend(
        4,
        Arc::new(BlockingBackend {
            entered: entered.clone(),
            release: release.clone(),
        }),
    );
    writer
        .write(|connection| {
            let policy_id: String = connection
                .query_row("SELECT id FROM rr_policy_versions ORDER BY version DESC LIMIT 1", [], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('role-detach','conversation_primary',?1,0,'responding','text','visual',1,'')", [&policy_id]).map_err(|error| error.to_string())?;
            connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('role-detach-step','role-detach',0,0,'actor','respond','running','fingerprint','{}',1)", []).map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("role root");
    let harness = serve_with(writer.clone(), service).await;
    let session = harness.ready_role_session("role-detach").await;
    let execution_ref = prepare_execution_ref(&harness, &session).await;
    let client = harness.client.clone();
    let url = harness.base.clone();
    let token = harness.token.clone();
    let session_for_task = session.clone();
    let request = tokio::spawn(async move {
        client
            .post(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", ACCEPT_BOTH)
            .header("Mcp-Session-Id", session_for_task)
            .json(&json!({
                "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"tools_invoke","arguments":{"executionRef":execution_ref,"arguments":{"q":"v"}}}
            }))
            .timeout(Duration::from_millis(100))
            .send()
            .await
    });
    let _entry = tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .expect("backend entered")
        .expect("permit");
    let _ = request.await.expect("request task");
    release.add_permits(1);
    assert_eq!(wait_for_invocation_status(&writer).await, "succeeded");
    for _ in 0..200 {
        let unsettled = writer
            .read_serialized(|connection| connection
                .query_row("SELECT count(*) FROM rr_tool_links WHERE root_id='role-detach' AND dispatch_state<>'settled'", [], |row| row.get::<_, i64>(0))
                .map_err(|error| error.to_string()))
            .expect("routing links");
        if unsettled == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("detached role owner did not settle its routing link");
}
#[tokio::test]
pub(super) async fn rr_29_tool_dispatch_update_then_settle() {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (writer, service) = ledger_with_backend(
        4,
        Arc::new(BlockingBackend {
            entered: entered.clone(),
            release: release.clone(),
        }),
    );
    writer
        .write(|connection| {
            let policy_id: String = connection
                .query_row("SELECT id FROM rr_policy_versions ORDER BY version DESC LIMIT 1", [], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            connection.execute("INSERT INTO rr_roots(root_id,conversation_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('role-update','conversation_primary',?1,0,'responding','text','visual',1,'')", [&policy_id]).map_err(|error| error.to_string())?;
            connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('role-update-step','role-update',0,0,'actor','respond','running','fingerprint','{}',1)", []).map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("role root");
    let harness = serve_with(writer.clone(), service).await;
    let session = harness.ready_role_session("role-update").await;
    let execution_ref = prepare_execution_ref(&harness, &session).await;
    let client = harness.client.clone();
    let url = harness.base.clone();
    let token = harness.token.clone();
    let request = tokio::spawn(async move {
        client
            .post(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", ACCEPT_BOTH)
            .header("Mcp-Session-Id", session)
            .json(&json!({
                "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"tools_invoke","arguments":{"executionRef":execution_ref,"arguments":{"q":"v"}}}
            }))
            .send()
            .await
    });
    let _entry = tokio::time::timeout(Duration::from_secs(5), entered.acquire())
        .await
        .expect("backend entered")
        .expect("permit");
    writer
        .write(|connection| {
            crate::role_routing::coordinator::apply(
                connection,
                "role-update",
                crate::role_routing::reducer::Event::InputBarrier,
                2,
            )?;
            Ok(())
        })
        .expect("condition update commits while owner runs");
    release.add_permits(1);
    let response = request
        .await
        .expect("request task")
        .expect("gateway response");
    assert!(response.status().is_success());
    assert_eq!(wait_for_invocation_status(&writer).await, "succeeded");
    let state = writer
        .read_serialized(|connection| {
            connection
                .query_row(
                    "SELECT r.phase||':'||l.dispatch_state||':'||
                       (SELECT count(*) FROM tool_selection_invocations)
                     FROM rr_roots r JOIN rr_tool_links l ON l.root_id=r.root_id
                     WHERE r.root_id='role-update'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .map_err(|error| error.to_string())
        })
        .expect("routing state");
    assert_eq!(state, "draining:settled:1");
}
#[tokio::test]
pub(super) async fn rr_21_sol_tool_roundtrip() {
    let (writer, service) = ledger(4);
    writer
        .write(|connection| {
            let policy_id: String = connection
                .query_row("SELECT id FROM rr_policy_versions ORDER BY version DESC LIMIT 1", [], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at) VALUES('sol-run','conversation_primary','conversation.respond','running','1')", []).map_err(|error| error.to_string())?;
            connection.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('sol-run','conversation_primary','sol-run',?1,0,'responding','text','visual',1,'')", [&policy_id]).map_err(|error| error.to_string())?;
            connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('sol-step','sol-run',0,0,'sol','respond','running','sol-fingerprint','{}',1)", []).map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("Sol step binding");
    let harness = serve_with(writer.clone(), service).await;
    // This role-scoped MCP session stands in for the injected SDK thread. It exercises the real
    // authenticated loopback gateway, catalog resolution, permit, owner, and routing ledger.
    let session = harness.ready_role_session("sol-run").await;
    let execution_ref = prepare_execution_ref(&harness, &session).await;
    let tool_result = harness
        .envelope(
            3,
            "tools_invoke",
            json!({"executionRef":execution_ref,"arguments":{"q":"decision"}}),
            &session,
        )
        .await;
    assert_eq!(tool_result.pointer("/ok"), Some(&json!(true)));
    writer
        .write(|connection| {
            let transaction = connection.transaction().map_err(|error| error.to_string())?;
            transaction.execute("INSERT INTO conversation_messages(id,conversation_id,role,content,created_at) VALUES('sol-answer','conversation_primary','assistant','answer from tool result','2')", []).map_err(|error| error.to_string())?;
            crate::role_routing::repository::accept_provider_turn(&transaction, "sol-run", "sol-answer", 2)?;
            transaction.commit().map_err(|error| error.to_string())?;
            Ok(())
        })
        .expect("host adopts SDK candidate");
    let receipt = writer
        .read_serialized(|connection| {
            connection.query_row(
                "SELECT r.phase||':'||m.content||':'||(SELECT count(*) FROM rr_tool_links l WHERE l.root_id=r.root_id AND l.dispatch_state<>'settled') FROM rr_roots r JOIN conversation_messages m ON m.id=r.result_message_id WHERE r.root_id='sol-run'",
                [],
                |row| row.get::<_,String>(0),
            ).map_err(|error| error.to_string())
        })
        .expect("roundtrip receipt");
    assert_eq!(receipt, "completed:answer from tool result:0");
    harness.server.shutdown().await;
}
#[tokio::test]
pub(super) async fn rr_38_normal_turn_specialist_returns_to_parent() {
    let (writer, service) = ledger(4);
    writer
        .write(|connection| {
            let policy_id: String = connection
                .query_row(
                    "SELECT id FROM rr_policy_versions ORDER BY version DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at) VALUES('specialist-run','conversation_primary','conversation.respond','running','1')", []).map_err(|error| error.to_string())?;
            connection.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('specialist-run','conversation_primary','specialist-run',?1,0,'responding','text','visual',1,'')", [&policy_id]).map_err(|error| error.to_string())?;
            connection.execute_batch(
                "INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES
                 ('specialist-parent-draft','specialist-run',0,0,'parent','respond','running','parent-fingerprint','{}',1),
                 ('specialist-step','specialist-run',0,1,'specialist','tool_specialist','planned','specialist-fingerprint','{}',NULL),
                 ('specialist-parent-final','specialist-run',0,2,'parent','respond','planned','parent-fingerprint','{}',NULL);",
            ).map_err(|error| error.to_string())?;
            crate::role_routing::repository::advance_provider_step(
                connection,
                "specialist-run",
                "private parent draft",
                2,
            )?;
            Ok(())
        })
        .expect("specialist plan");

    let request = crate::role_routing::tool_specialist::SpecialistRequest {
        tool_name: "tools_search".into(),
        arguments: json!({"intent":"find a note for the parent","limit":1}),
    };
    let cancellation = RunCancellation::default();
    let envelope = crate::role_routing::tool_specialist::execute_for_root(
        &service,
        &writer,
        "conversation_primary",
        &gateway::RoleStepBinding {
            root_id: "specialist-run",
            step_id: "specialist-step",
            revision: 0,
            attempt_started_at_ms: 2,
            config_fingerprint: "specialist-fingerprint",
        },
        None,
        &request,
        true,
        &[
            "tools_search".into(),
            "tools_describe".into(),
            "tools_invoke".into(),
        ],
        &cancellation,
    )
    .await
    .expect("specialist request reaches host gateway");
    assert_eq!(envelope.get("ok"), Some(&json!(true)));
    let duplicate = crate::role_routing::tool_specialist::execute_for_root(
        &service,
        &writer,
        "conversation_primary",
        &gateway::RoleStepBinding {
            root_id: "specialist-run",
            step_id: "specialist-step",
            revision: 0,
            attempt_started_at_ms: 2,
            config_fingerprint: "specialist-fingerprint",
        },
        None,
        &request,
        true,
        &[
            "tools_search".into(),
            "tools_describe".into(),
            "tools_invoke".into(),
        ],
        &cancellation,
    )
    .await
    .expect("duplicate returns a host envelope");
    assert_eq!(
        duplicate.pointer("/error/code"),
        Some(&json!("operation-not-retryable")),
        "a settled or unknown operation remains single-owner and the parent cannot retry it"
    );

    let envelope_text = serde_json::to_string(&envelope).expect("tool envelope");
    writer
        .write(|connection| {
            assert!(crate::role_routing::repository::advance_provider_step(
                connection,
                "specialist-run",
                &envelope_text,
                3,
            )?);
            let state: String = connection
                .query_row(
                    "SELECT
                     (SELECT status FROM rr_steps WHERE id='specialist-step')||':'||
                     (SELECT status FROM rr_steps WHERE id='specialist-parent-final')||':'||
                     (SELECT count(*) FROM rr_tool_links WHERE root_id='specialist-run' AND dispatch_state='settled')||':'||
                     COALESCE((SELECT result_message_id FROM rr_roots WHERE root_id='specialist-run'),'none')",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            assert_eq!(state, "succeeded:running:1:none");
            Ok(())
        })
        .expect("tool result returns only to parent");
}
#[tokio::test]
pub(super) async fn h07_cancelling_one_session_does_not_cancel_another_with_the_same_id() {
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let (writer, service) = ledger_with_backend(
        4,
        Arc::new(BlockingBackend {
            entered: entered.clone(),
            release: release.clone(),
        }),
    );
    let harness = serve_with(writer.clone(), service).await;
    let first = harness.ready_session().await;
    let second = harness.ready_session().await;
    let first_ref = prepare_execution_ref(&harness, &first).await;
    let second_ref = prepare_execution_ref(&harness, &second).await;

    // Both sessions run a call with the same typed request id at the same time.
    let first_call = spawn_tool_call(&harness, &first, 7, &first_ref);
    let second_call = spawn_tool_call(&harness, &second, 7, &second_ref);
    for _ in 0..2 {
        let _entry = tokio::time::timeout(Duration::from_secs(5), entered.acquire())
            .await
            .expect("backend entered")
            .expect("permit");
    }

    // Cancel id 7 in the first session only.
    let (status, _) = harness
        .rpc(
            &json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": 7 }
            }),
            Some(&first),
        )
        .await;
    assert_eq!(status, reqwest::StatusCode::ACCEPTED);
    release.add_permits(2);
    let _ = tokio::time::timeout(Duration::from_secs(5), first_call).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), second_call).await;

    let statuses: Vec<String> = writer
        .read_serialized(|connection| {
            let mut statement = connection
                .prepare("SELECT technical_status FROM tool_selection_invocations ORDER BY rowid")
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())
        })
        .expect("statuses");
    assert!(
        statuses.contains(&"cancelled".to_string()),
        "the targeted session must be cancelled: {statuses:?}"
    );
    assert!(
        statuses.contains(&"succeeded".to_string()),
        "the other session's same id must keep running: {statuses:?}"
    );
}
#[tokio::test]
pub(super) async fn h11_parallel_intents_use_request_local_scenarios() {
    let harness = harness(4).await;
    let session = harness.ready_session().await;
    // Two searches with different intents run at the same time in one session. Each decision must
    // record its own intent; neither may observe the other's scenario.
    let first = {
        let harness = &harness;
        let session = session.clone();
        async move {
            harness
                .envelope(
                    11,
                    "tools_search",
                    json!({ "intent": "alpha decision records" }),
                    &session,
                )
                .await
        }
    };
    let second = {
        let harness = &harness;
        let session = session.clone();
        async move {
            harness
                .envelope(
                    12,
                    "tools_search",
                    json!({ "intent": "beta meeting minutes" }),
                    &session,
                )
                .await
        }
    };
    let (left, right) = tokio::join!(first, second);
    assert_eq!(left.pointer("/ok"), Some(&json!(true)));
    assert_eq!(right.pointer("/ok"), Some(&json!(true)));

    let scenarios: Vec<String> = harness
        .writer
        .read_serialized(|connection| {
            let mut statement = connection
                .prepare("SELECT scenario_json FROM tool_selection_decisions ORDER BY created_at")
                .map_err(|error| error.to_string())?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| error.to_string())?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())
        })
        .expect("scenarios");
    let joined = scenarios.join("\n");
    assert!(joined.contains("alpha decision records"), "{joined}");
    assert!(joined.contains("beta meeting minutes"), "{joined}");
}
// ---------------------------------------------------------------------------------------------
// H08: explicit cancellation, DELETE, shutdown and task panic all reach a terminal state
// ---------------------------------------------------------------------------------------------

pub(crate) async fn prepare_execution_ref(harness: &D5, session: &str) -> String {
    let search = harness
        .envelope(
            1,
            "tools_search",
            json!({ "intent": "search notes" }),
            session,
        )
        .await;
    let candidate = search
        .pointer("/data/candidates/0/candidateRef")
        .and_then(Value::as_str)
        .expect("candidate")
        .to_string();
    let describe = harness
        .envelope(
            2,
            "tools_describe",
            json!({ "candidateRef": candidate }),
            session,
        )
        .await;
    describe
        .pointer("/data/executionRef")
        .and_then(Value::as_str)
        .expect("execution ref")
        .to_string()
}
pub(crate) fn spawn_tool_call(
    harness: &D5,
    session: &str,
    id: i64,
    execution_ref: &str,
) -> tokio::task::JoinHandle<()> {
    let client = harness.client.clone();
    let url = harness.base.clone();
    let token = harness.token.clone();
    let session = session.to_string();
    let execution_ref = execution_ref.to_string();
    tokio::spawn(async move {
        let _ = client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", ACCEPT_BOTH)
            .header("Mcp-Session-Id", session)
            .json(&json!({
                "jsonrpc": "2.0", "id": id, "method": "tools/call",
                "params": { "name": "tools_invoke", "arguments": {
                    "executionRef": execution_ref, "arguments": { "q": "v" } } }
            }))
            .send()
            .await;
    })
}
pub(crate) async fn wait_for_invocation_status(writer: &SqliteWriter) -> String {
    for _ in 0..600 {
        let status = writer
            .read_serialized(|connection| {
                connection
                    .query_row(
                        "SELECT technical_status FROM tool_selection_invocations ORDER BY rowid DESC LIMIT 1",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(|error| error.to_string())
            })
            .unwrap_or_default();
        if status != "running" && !status.is_empty() {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    "running".to_string()
}
