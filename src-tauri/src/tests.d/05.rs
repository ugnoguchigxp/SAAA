#[cfg(unix)]
#[test]
fn codex_app_server_contract_covers_start_stream_resume_and_cancel() {
    let _lock = crate::test_environment::codex_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("temporary directory");
    let executable = directory.path().join("codex-fixture.py");
    let log_path = directory.path().join("requests.jsonl");
    let quoted_log_path =
        serde_json::to_string(&log_path.to_string_lossy()).expect("fixture log path encodes");
    let fixture = format!(
        r#"#!/usr/bin/env python3
import json, sys
log_path = {quoted_log_path}
scenario = "normal"
with open(log_path, "a", encoding="utf-8") as log:
    log.write(json.dumps({{"argv": sys.argv[1:]}}) + "\n")
for line in sys.stdin:
    message = json.loads(line)
    with open(log_path, "a", encoding="utf-8") as log:
        log.write(json.dumps(message) + "\n")
    request_id = message.get("id")
    method = message.get("method")
    if request_id == 1:
        print(json.dumps({{"id": 1, "result": {{}}}}), flush=True)
    elif request_id == 2:
        scenario = message.get("params", {{}}).get("model", "normal")
        if scenario != "thread-hang":
            thread_id = "x" * 161 if scenario == "invalid-thread-id" else "fixture-thread"
            print(json.dumps({{"id": 2, "result": {{"thread": {{"id": thread_id}}}}}}), flush=True)
    elif request_id == 3:
        text = message.get("params", {{}}).get("input", [{{}}])[0].get("text", "")
        if scenario != "turn-hang":
            turn_id = "x" * 161 if scenario == "invalid-turn-id" else "fixture-turn"
            print(json.dumps({{"id": 3, "result": {{"turn": {{"id": turn_id}}}}}}), flush=True)
        if scenario == "malformed":
            print("{{not-json", flush=True)
        elif scenario == "provider-error":
            print(json.dumps({{"method": "error", "params": {{"message": "SAAA_PRIVATE_PROVIDER_DETAIL"}}}}), flush=True)
        elif scenario == "terminal-failed":
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "failed", "error": {{"message": "SAAA_PRIVATE_TERMINAL_DETAIL"}}}}}}}}), flush=True)
        elif scenario == "approval":
            print(json.dumps({{"id": 99, "method": "item/requestApproval", "params": {{}}}}), flush=True)
        elif scenario in ["fileChange", "mcpToolCall", "dynamicToolCall", "webSearch"]:
            print(json.dumps({{"method": "item/started", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "forbidden_1", "type": scenario}}}}}}), flush=True)
        elif scenario == "terminal-hang":
            print(json.dumps({{"method": "item/started", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage"}}}}}}), flush=True)
            print(json.dumps({{"method": "item/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage", "text": "SAAA_TERMINAL_WAIT"}}}}}}), flush=True)
        elif scenario == "foreign":
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"threadId": "other", "turnId": "other", "delta": "foreign"}}}}), flush=True)
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"delta": "unscoped"}}}}), flush=True)
        elif scenario == "duplicate":
            started = {{"method": "item/started", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage"}}}}}}
            completed = {{"method": "item/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "item": {{"id": "message_1", "type": "agentMessage", "text": "SAAA_DUPLICATE_OK"}}}}}}
            print(json.dumps(started), flush=True)
            print(json.dumps(started), flush=True)
            print(json.dumps(completed), flush=True)
            print(json.dumps(completed), flush=True)
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "completed"}}}}}}), flush=True)
        elif scenario == "response-too-large":
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "delta": "x" * 64001}}}}), flush=True)
        elif scenario == "stream-too-large":
            print(json.dumps({{"method": "unknown", "params": {{"blob": "x" * (4 * 1024 * 1024)}}}}), flush=True)
        elif scenario == "child-exit":
            sys.exit(3)
        elif scenario not in ["progress-hang", "hard-hang", "cancel-no-response"] and "CANCEL" not in text:
            reply = "SAAA_RESUMED" if method == "turn/start" and "RESUME" in text else "SAAA_OK"
            print(json.dumps({{"method": "item/agentMessage/delta", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "delta": reply}}}}), flush=True)
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "completed"}}}}}}), flush=True)
    elif method == "turn/interrupt":
        if scenario != "cancel-no-response":
            print(json.dumps({{"id": request_id, "result": {{}}}}), flush=True)
            print(json.dumps({{"method": "turn/completed", "params": {{"threadId": "fixture-thread", "turnId": "fixture-turn", "turn": {{"id": "fixture-turn", "threadId": "fixture-thread", "status": "interrupted"}}}}}}), flush=True)
"#,
    );
    fs::write(&executable, fixture).expect("fixture writes");
    let mut permissions = fs::metadata(&executable)
        .expect("fixture metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).expect("fixture becomes executable");

    let _codex_environment = crate::test_environment::EnvGuard::set("SAAA_CODEX_PATH", &executable);
    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let received_for_channel = received.clone();
    let channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            received_for_channel.lock().expect("event lock").push(value);
        }
        Ok(())
    });
    let cancellation = RunCancellation::default();
    let first = run_codex_turn_process(
        "run-start",
        "START",
        directory.path(),
        "gpt-fixture",
        None,
        10_000,
        &channel,
        &cancellation,
    )
    .expect("start turn succeeds");
    assert_eq!(first.thread_id, "fixture-thread");
    assert_eq!(first.content, "SAAA_OK");
    let resumed = run_codex_turn_process(
        "run-resume",
        "RESUME",
        directory.path(),
        "gpt-fixture",
        Some(&first.thread_id),
        10_000,
        &channel,
        &cancellation,
    )
    .expect("resume turn succeeds");
    assert_eq!(resumed.content, "SAAA_RESUMED");
    assert_eq!(
        run_codex_turn_process(
            "run-invalid-resume",
            "RESUME",
            directory.path(),
            "gpt-fixture",
            Some(&"x".repeat(161)),
            10_000,
            &channel,
            &cancellation,
        )
        .expect_err("invalid persisted thread id fails closed")
        .code,
        runtime::contracts::RunFailureCode::ProtocolError
    );
    let cancelled = Arc::new(RunCancellation::default());
    let cancel_trigger = cancelled.clone();
    let cancel_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        cancel_trigger.cancel();
    });
    let cancellation_result = run_codex_turn_process(
        "run-cancel",
        "CANCEL",
        directory.path(),
        "gpt-fixture",
        Some(&first.thread_id),
        10_000,
        &channel,
        &cancelled,
    );
    cancel_thread.join().expect("cancel trigger joins");

    let policy = runtime::contracts::RunSupervisionPolicy {
        request_timeout_ms: 200,
        progress_idle_timeout_ms: 40,
        terminal_gap_timeout_ms: 30,
        interrupt_grace_ms: 20,
        hard_timeout_ms: 200,
    };
    let interrupt_count = || {
        fs::read_to_string(&log_path)
            .expect("fixture log loads")
            .matches("\"method\": \"turn/interrupt\"")
            .count()
    };
    let run_scenario = |scenario: &str,
                        scenario_policy: runtime::contracts::RunSupervisionPolicy,
                        cancellation: &RunCancellation| {
        let before = interrupt_count();
        let result = run_codex_turn_process_with_policy(
            "run-scenario",
            scenario,
            directory.path(),
            scenario,
            None,
            scenario_policy,
            &channel,
            cancellation,
        );
        assert!(
            interrupt_count().saturating_sub(before) <= 1,
            "scenario sent more than one interrupt: {scenario}"
        );
        result
    };
    for (scenario, expected) in [
        (
            "thread-hang",
            runtime::contracts::RunFailureCode::RequestTimeout,
        ),
        (
            "turn-hang",
            runtime::contracts::RunFailureCode::RequestTimeout,
        ),
        (
            "invalid-thread-id",
            runtime::contracts::RunFailureCode::ProtocolError,
        ),
        (
            "invalid-turn-id",
            runtime::contracts::RunFailureCode::ProtocolError,
        ),
        (
            "progress-hang",
            runtime::contracts::RunFailureCode::ProgressTimeout,
        ),
        (
            "terminal-hang",
            runtime::contracts::RunFailureCode::TerminalTimeout,
        ),
        (
            "malformed",
            runtime::contracts::RunFailureCode::ProtocolError,
        ),
        (
            "provider-error",
            runtime::contracts::RunFailureCode::ProviderError,
        ),
        (
            "terminal-failed",
            runtime::contracts::RunFailureCode::ProviderError,
        ),
        (
            "approval",
            runtime::contracts::RunFailureCode::PolicyViolation,
        ),
        (
            "child-exit",
            runtime::contracts::RunFailureCode::ChildExited,
        ),
        (
            "foreign",
            runtime::contracts::RunFailureCode::ProgressTimeout,
        ),
        (
            "response-too-large",
            runtime::contracts::RunFailureCode::ResponseTooLarge,
        ),
        (
            "stream-too-large",
            runtime::contracts::RunFailureCode::ResponseTooLarge,
        ),
    ] {
        // Protocol rejection tests must not accidentally assert Python startup latency.
        // Keep the short deadlines only for scenarios explicitly testing timeout behavior.
        let scenario_policy = if matches!(
            expected,
            runtime::contracts::RunFailureCode::RequestTimeout
                | runtime::contracts::RunFailureCode::ProgressTimeout
                | runtime::contracts::RunFailureCode::TerminalTimeout
        ) {
            policy
        } else {
            runtime::contracts::RunSupervisionPolicy {
                request_timeout_ms: 5_000,
                progress_idle_timeout_ms: 5_000,
                terminal_gap_timeout_ms: 5_000,
                hard_timeout_ms: 10_000,
                ..policy
            }
        };
        let failure = run_scenario(scenario, scenario_policy, &RunCancellation::default())
            .expect_err("scenario must fail");
        assert_eq!(failure.code, expected, "scenario: {scenario}");
        assert!(!failure.message.contains("SAAA_PRIVATE_"));
    }
    let hard_policy = runtime::contracts::RunSupervisionPolicy {
        progress_idle_timeout_ms: 200,
        hard_timeout_ms: 30,
        ..policy
    };
    assert_eq!(
        run_scenario("hard-hang", hard_policy, &RunCancellation::default())
            .expect_err("hard timeout must fail")
            .code,
        runtime::contracts::RunFailureCode::HardTimeout
    );
    for forbidden in ["fileChange", "mcpToolCall", "dynamicToolCall", "webSearch"] {
        assert_eq!(
            run_scenario(forbidden, policy, &RunCancellation::default())
                .expect_err("forbidden item must fail")
                .code,
            runtime::contracts::RunFailureCode::PolicyViolation
        );
    }
    let duplicate = run_scenario("duplicate", policy, &RunCancellation::default())
        .expect("duplicate notifications must not break completion");
    assert_eq!(duplicate.content, "SAAA_DUPLICATE_OK");

    let unresponsive_cancel = Arc::new(RunCancellation::default());
    let cancel_trigger = unresponsive_cancel.clone();
    let cancel_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        cancel_trigger.cancel();
    });
    let unresponsive = run_scenario("cancel-no-response", policy, &unresponsive_cancel)
        .expect_err("unresponsive cancellation must finish");
    cancel_thread.join().expect("cancel trigger joins");
    assert_eq!(
        unresponsive.code,
        runtime::contracts::RunFailureCode::UserCancelled
    );
    let interrupts_before_atomic = interrupt_count();

    let mut connection = Connection::open_in_memory().expect("database opens");
    initialize_database(&connection).expect("database initializes");
    connection
        .execute(
            "INSERT INTO conversations(id,title,task_mode,created_at,updated_at)
                 VALUES('coding-atomic','Atomic','coding','1','1')",
            [],
        )
        .expect("coding conversation inserts");
    let mut documents = default_settings_input();
    let codex = documents
        .iter_mut()
        .find(|document| document.namespace == "providers.agent")
        .expect("Codex settings exist");
    codex.value_json["enabled"] = Value::Bool(true);
    codex.value_json["model"] = Value::String("normal".to_string());
    save_settings_documents_to_connection(&mut connection, &documents)
        .expect("Codex settings save");
    let state = app_state(connection);
    let committed_terminal_states =
        Arc::new(Mutex::new(
            Vec::<(String, String, String, Option<String>)>::new(),
        ));
    let committed_terminal_states_for_channel = committed_terminal_states.clone();
    let database_for_channel = state.sqlite_writer.clone();
    let atomic_channel: tauri::ipc::Channel<RuntimeEvent> = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(value) = body {
            let event: Value = serde_json::from_str(&value).expect("runtime event decodes");
            let event_type = event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if matches!(event_type, "messageCompleted" | "cancelled" | "failed") {
                let run_id = event
                    .get("runId")
                    .and_then(Value::as_str)
                    .expect("terminal event has run id");
                let database = database_for_channel.lock().expect("database lock");
                let (status, failure_code): (String, Option<String>) = database
                    .query_row(
                        "SELECT status,failure_code FROM runtime_runs WHERE id=?1",
                        [run_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .expect("committed run reads from terminal callback");
                committed_terminal_states_for_channel
                    .lock()
                    .expect("terminal state lock")
                    .push((
                        run_id.to_string(),
                        event_type.to_string(),
                        status,
                        failure_code,
                    ));
            }
        }
        Ok(())
    });
    let atomic_input = StartTurnInput {
        run_id: "run-atomic".to_string(),
        conversation_id: "coding-atomic".to_string(),
        content: "ATOMIC".to_string(),
        workspace_path: Some(directory.path().to_string_lossy().into_owned()),
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".to_string(),
        presentation_mode: "visual".to_string(),
    };
    tauri::async_runtime::block_on(execute_turn(
        &state,
        &atomic_input,
        &atomic_channel,
        Arc::new(RunCancellation::default()),
        Some(policy),
    ))
    .expect("atomic Codex turn succeeds");
    assert_eq!(interrupt_count(), interrupts_before_atomic);
    assert_eq!(
        committed_terminal_states
            .lock()
            .expect("terminal state lock")
            .as_slice(),
        [(
            "run-atomic".to_string(),
            "messageCompleted".to_string(),
            "completed".to_string(),
            None
        )]
    );
    let database = state.sqlite_writer.lock().expect("database lock");
    let (status, supervisor_version, assistant_count): (String, String, i64) = database
        .query_row(
            "SELECT r.status,r.supervisor_version,
                        (SELECT COUNT(*) FROM conversation_messages
                         WHERE conversation_id='coding-atomic' AND role='assistant')
                 FROM runtime_runs r WHERE r.id='run-atomic'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("atomic state reads");
    assert_eq!(status, "completed");
    assert_eq!(supervisor_version, runtime::contracts::SUPERVISOR_VERSION);
    assert_eq!(assistant_count, 1);
    drop(database);

    let run_atomic_scenario = |run_id: &str, scenario: &str, cancellation: Arc<RunCancellation>| {
        {
            let mut database = state.sqlite_writer.lock().expect("database lock");
            let mut documents = default_settings_input();
            let codex = documents
                .iter_mut()
                .find(|document| document.namespace == "providers.agent")
                .expect("Codex settings exist");
            codex.value_json["enabled"] = Value::Bool(true);
            codex.value_json["model"] = Value::String(scenario.to_string());
            save_settings_documents_to_connection(&mut database, &documents)
                .expect("scenario Codex settings save");
        }
        let input = StartTurnInput {
            run_id: run_id.to_string(),
            conversation_id: "coding-atomic".to_string(),
            content: scenario.to_string(),
            workspace_path: Some(directory.path().to_string_lossy().into_owned()),
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        tauri::async_runtime::block_on(execute_turn(
            &state,
            &input,
            &atomic_channel,
            cancellation,
            Some(policy),
        ))
    };

    let cancellation = Arc::new(RunCancellation::default());
    let cancel_trigger = cancellation.clone();
    let cancel_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        cancel_trigger.cancel();
    });
    let interrupts_before_cancel = interrupt_count();
    let cancelled = run_atomic_scenario("run-atomic-cancel", "cancel-no-response", cancellation)
        .expect_err("cancel scenario must cancel");
    cancel_thread.join().expect("atomic cancel trigger joins");
    assert!(interrupt_count().saturating_sub(interrupts_before_cancel) <= 1);
    assert_eq!(
        cancelled.code,
        runtime::contracts::RunFailureCode::UserCancelled
    );
    let interrupts_before_progress = interrupt_count();
    let progress = run_atomic_scenario(
        "run-atomic-progress",
        "progress-hang",
        Arc::new(RunCancellation::default()),
    )
    .expect_err("progress scenario must time out");
    assert_eq!(
        progress.code,
        runtime::contracts::RunFailureCode::ProgressTimeout
    );
    assert_eq!(interrupt_count() - interrupts_before_progress, 1);
    let interrupts_before_policy = interrupt_count();
    let policy_violation = run_atomic_scenario(
        "run-atomic-policy",
        "fileChange",
        Arc::new(RunCancellation::default()),
    )
    .expect_err("policy scenario must fail");
    assert_eq!(
        policy_violation.code,
        runtime::contracts::RunFailureCode::PolicyViolation
    );
    assert_eq!(interrupt_count() - interrupts_before_policy, 1);

    let terminal_states = committed_terminal_states
        .lock()
        .expect("terminal state lock");
    for expected in [
        (
            "run-atomic-cancel",
            "cancelled",
            "cancelled",
            Some("user-cancelled"),
        ),
        (
            "run-atomic-progress",
            "failed",
            "failed",
            Some("progress-timeout"),
        ),
        (
            "run-atomic-policy",
            "failed",
            "failed",
            Some("policy-violation"),
        ),
    ] {
        let matching = terminal_states
            .iter()
            .filter(|entry| entry.0 == expected.0)
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 1, "one terminal event for {}", expected.0);
        assert_eq!(matching[0].1, expected.1);
        assert_eq!(matching[0].2, expected.2);
        assert_eq!(matching[0].3.as_deref(), expected.3);
    }
    drop(terminal_states);
    assert!(cancellation_result.is_err());
    let log = fs::read_to_string(log_path).expect("fixture log loads");
    assert!(log.contains("thread/start"));
    assert!(log.contains("thread/resume"));
    assert!(log.contains("turn/interrupt"));
    assert!(log.contains("read-only"));
    assert!(log.contains("\"approvalPolicy\": \"never\""));
    assert!(log.contains("\"network_access\": false"));
    assert!(received
        .lock()
        .expect("event lock")
        .iter()
        .any(|event| { event.contains("SAAA_OK") && event.contains("\"type\":\"delta\"") }));
}
