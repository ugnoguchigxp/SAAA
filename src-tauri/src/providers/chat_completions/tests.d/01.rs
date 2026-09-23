#[derive(Default, Clone)]
struct Sink(Arc<Mutex<Vec<String>>>);
impl RuntimeEventSender for Sink {
    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        if let RuntimeEvent::Delta { text, .. } = event {
            self.0.lock().unwrap().push(text);
        }
        Ok(())
    }
}
#[derive(Clone)]
struct ScopeChangingSink {
    writer: Arc<crate::persistence::SqliteWriter>,
}
impl RuntimeEventSender for ScopeChangingSink {
    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }

    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        if matches!(event, RuntimeEvent::Delta { .. }) {
            self.writer
                .write(|connection| {
                    connection
                        .execute(
                            "UPDATE context_scope_epochs SET epoch=epoch+1 WHERE scope_key='scope'",
                            [],
                        )
                        .map_err(crate::database_error)?;
                    Ok(())
                })
                .expect("scope mutation succeeds");
        }
        Ok(())
    }
}
fn chunk(delta: Value, finish: Value) -> String {
    format!(
        "data: {}\r\n\r\n",
        json!({"model":"fixture", "choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    )
}
fn input() -> crate::StartTurnInput {
    crate::StartTurnInput {
        run_id: "http_fixture".into(),
        conversation_id: "fixture".into(),
        content: "hello".into(),
        workspace_path: None,
        retry_input_message_id: None,
        source_id: None,
        scope_refs: Vec::new(),
        input_origin: "text".into(),
        presentation_mode: "visual".into(),
    }
}
async fn fixture(
    responses: Vec<(u16, String, u64)>,
) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/proxy/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body, delay) in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let (end, length) = loop {
                let mut buffer = [0; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&bytes[..end]);
                    assert!(head.starts_with("POST /proxy/v1/chat/completions HTTP/1.1"));
                    assert!(!head.contains("Last-Event-ID"));
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|v| v.parse::<usize>().ok())
                        })
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < end + length {
                let mut buffer = [0; 4096];
                let n = socket.read(&mut buffer).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buffer[..n]);
            }
            let request: Value = serde_json::from_slice(&bytes[end..end + length]).unwrap();
            assert!(request["stream"].is_boolean());
            assert_eq!(request["model"], "fixture");
            requests.push(request);
            let media = if body.starts_with('{') {
                "application/json"
            } else {
                "text/event-stream"
            };
            let header = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: {media}\r\nContent-Length: {}\r\nRetry-After: 0\r\nConnection: close\r\n\r\n", body.len());
            if socket.write_all(header.as_bytes()).await.is_err() {
                continue;
            }
            for byte in body.as_bytes() {
                if socket.write_all(&[*byte]).await.is_err() {
                    break;
                }
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            }
        }
        requests
    });
    (endpoint, task)
}
async fn invoke(
    endpoint: &str,
    sink: &Sink,
    cancellation: Arc<crate::RunCancellation>,
) -> Result<String, Error> {
    run(
        endpoint,
        None,
        "fixture",
        &[],
        5000,
        ModelStreamContext {
            reasoning_effort: "medium",
            max_output_tokens: 64,
            input: &input(),
            on_event: sink,
            cancellation,
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
    )
    .await
}
#[tokio::test]
async fn socket_sse_without_models_sessions_or_websocket() {
    let body = chunk(json!({"role":"assistant"}), Value::Null)
        + &chunk(json!({"content":"日本語", "reasoning_content":"hidden"}), Value::Null)
        + &chunk(json!({}), json!("stop"))
        + "data: {\"choices\":[],\"usage\":{\"total_tokens\":8},\"extension\":true}\n\ndata: [DONE]\n\n";
    let (url, server) = fixture(vec![(200, body, 0)]).await;
    let sink = Sink::default();
    assert_eq!(invoke(&url, &sink, Arc::default()).await.unwrap(), "日本語");
    assert_eq!(*sink.0.lock().unwrap(), ["日本語"]);
    server.await.unwrap();
}
#[tokio::test]
async fn rejects_eof_bad_json_model_mismatch_and_length() {
    for (body, kind, started) in [
        (
            chunk(json!({"content":"partial"}), Value::Null),
            Failure::ResponseInterrupted,
            true,
        ),
        ("data: {broken}\n\n".into(), Failure::Protocol, false),
        (
            chunk(json!({"role":"assistant"}), Value::Null)
                + &chunk(json!({}), json!("stop")).replace("fixture", "wrong"),
            Failure::Contract,
            false,
        ),
        (
            chunk(json!({"content":"cut"}), json!("length")) + "data: [DONE]\n\n",
            Failure::PartialOutput,
            true,
        ),
    ] {
        let (url, server) = fixture(vec![(200, body, 0)]).await;
        assert_eq!(
            invoke(&url, &Sink::default(), Arc::default()).await,
            Err(Error::failed(kind, started))
        );
        server.await.unwrap();
    }
}
#[tokio::test]
async fn retry_is_bounded_and_authentication_is_terminal() {
    for (statuses, expected) in [
        (vec![429, 503, 200], None),
        (vec![401], Some(Failure::Authentication)),
        (vec![503, 503, 503], Some(Failure::Unavailable)),
    ] {
        let body = chunk(json!({"content":"ok"}), json!("stop")) + "data: [DONE]\n\n";
        let count = statuses.len();
        let (url, server) = fixture(
            statuses
                .into_iter()
                .map(|status| (status, body.clone(), 0))
                .collect(),
        )
        .await;
        let result = invoke(&url, &Sink::default(), Arc::default()).await;
        if let Some(kind) = expected {
            assert_eq!(result, Err(Error::failed(kind, false)));
        } else {
            assert_eq!(result, Ok("ok".into()));
        }
        assert_eq!(server.await.unwrap().len(), count);
    }
}
#[tokio::test]
async fn cancellation_drops_waiting_request_and_never_emits_late_output() {
    let cancellation = Arc::new(crate::RunCancellation::default());
    cancellation.cancel();
    assert!(matches!(
        invoke("http://127.0.0.1:1/v1", &Sink::default(), cancellation).await,
        Err(Error::Cancelled {
            output_started: false
        })
    ));
    let body = chunk(json!({"content":"late"}), json!("stop")) + "data: [DONE]\n\n";
    let (url, server) = fixture(vec![(200, body, 2)]).await;
    let cancellation = Arc::new(crate::RunCancellation::default());
    let signal = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        signal.cancel();
    });
    let sink = Sink::default();
    assert!(matches!(
        invoke(&url, &sink, cancellation).await,
        Err(Error::Cancelled { .. })
    ));
    assert!(sink.0.lock().unwrap().is_empty());
    server.await.unwrap();
}
#[tokio::test]
async fn scope_change_while_provider_finishes_rejects_the_late_result_with_a_context_code() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    crate::persistence::schema::initialize_database(&connection).unwrap();
    connection
        .execute(
            "INSERT INTO conversations(id,task_mode,created_at,updated_at)
             VALUES('fixture','conversation','1','1')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO conversation_messages VALUES('http-source','fixture','user','hello','1')",
            [],
        )
        .unwrap();
    connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,input_message_id,started_at) VALUES('http_fixture','fixture','conversation.respond','running','http-source','1')", []).unwrap();
    connection.execute("INSERT INTO context_scopes(scope_key,kind,opaque_id,state,created_at) VALUES('scope','project','opaque','active','1')", []).unwrap();
    connection
        .execute(
            "INSERT INTO context_scope_epochs(scope_key,epoch) VALUES('scope',0)",
            [],
        )
        .unwrap();
    connection.execute("INSERT INTO runtime_scope_resolutions(run_id,status,focus_scope_key,scope_digest,resolved_at) VALUES('http_fixture','resolved','scope',?1,'1')", [&"d".repeat(64)]).unwrap();
    connection.execute("INSERT INTO runtime_run_scopes(run_id,scope_key,relation,source,epoch) VALUES('http_fixture','scope','current','runtime',0)", []).unwrap();
    let state = crate::test_support::app_state(connection);
    let session = crate::begin_provider_session(
        &state,
        "http_fixture",
        "fixture",
        "openai-compatible",
        &"a".repeat(64),
    )
    .unwrap();
    let body = chunk(json!({"content":"late"}), Value::Null)
        + &chunk(json!({}), json!("stop"))
        + "data: [DONE]\n\n";
    let (endpoint, server) = fixture(vec![(200, body, 0)]).await;
    let input = input();
    let history = [ConversationMessage {
        id: "http-source".into(),
        conversation_id: "fixture".into(),
        role: "user".into(),
        content: "hello".into(),
        created_at: "1".into(),
        parts: None,
    }];
    let sink = ScopeChangingSink {
        writer: state.sqlite_writer.clone(),
    };
    let result = run(
        &endpoint,
        None,
        "fixture",
        &history,
        5_000,
        ModelStreamContext {
            reasoning_effort: "medium",
            max_output_tokens: 64,
            input: &input,
            on_event: &sink,
            cancellation: Arc::default(),
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: Some(crate::ProviderOutputPersistence {
                state: &state,
                session_id: &session,
                world: None,
            }),
        },
    )
    .await;
    assert_eq!(
        result,
        Err(Error::failed(Failure::ContextScopeChanged, true))
    );
    assert_eq!(server.await.unwrap().len(), 1);
}
#[test]
fn assembles_interleaved_tool_calls_and_requires_done() {
    let mut completion = chunks::Completion::default();
    for delta in [
        json!({"tool_calls":[{"index":1,"id":"b","function":{"name":"second","arguments":"{\"b\":"}},{"index":0,"id":"a","function":{"name":"first","arguments":"{"}}]}),
        json!({"tool_calls":[{"index":0,"function":{"arguments":"\"a\":1}"}},{"index":1,"function":{"arguments":"2}"}}]}),
    ] {
        completion
            .absorb(
                &json!({"choices":[{"index":0,"delta":delta}]}).to_string(),
                "fixture",
            )
            .unwrap();
    }
    completion
        .absorb(
            &json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}).to_string(),
            "fixture",
        )
        .unwrap();
    assert_eq!(completion.complete(), Err(Failure::Protocol));
    completion.absorb("[DONE]", "fixture").unwrap();
    let calls = completion.complete().unwrap();
    assert_eq!(calls[0].id, "a");
    assert_eq!(calls[0].arguments, "{\"a\":1}");
    assert_eq!(calls[1].id, "b");
    assert_eq!(calls[1].arguments, "{\"b\":2}");
}
#[tokio::test]
async fn json_probe_uses_the_same_http_url_and_finish_validation_without_tools() {
    for reason in ["stop", "length"] {
        let body = json!({"model":"fixture","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":reason}]}).to_string();
        let (endpoint, server) = fixture(vec![(200, body, 0)]).await;
        let result = run_mode(
            &endpoint,
            None,
            "fixture",
            &[],
            5000,
            ModelStreamContext {
                reasoning_effort: "provider-default",
                max_output_tokens: 64,
                input: &input(),
                on_event: &Sink::default(),
                cancellation: Arc::default(),
                context_health: "green",
                context_sources: &[],
                context_omissions: &[],
                output_persistence: None,
            },
            RequestMode::JsonProbe,
        )
        .await;
        if reason == "stop" {
            assert_eq!(result, Ok("ok".into()));
        } else {
            assert_eq!(result, Err(Error::failed(Failure::PartialOutput, true)));
        }
        let requests = server.await.unwrap();
        assert_eq!(requests[0]["stream"], false);
        assert!(requests[0].get("tools").is_none());
        assert!(requests[0].get("reasoning_effort").is_none());
    }
}
#[derive(Clone)]
struct CancelAfterContent {
    sink: Sink,
    cancellation: Arc<crate::RunCancellation>,
}
impl RuntimeEventSender for CancelAfterContent {
    fn send(&self, event: RuntimeEvent) -> tauri::Result<()> {
        if matches!(event, RuntimeEvent::Delta { .. }) {
            self.cancellation.cancel();
        }
        self.sink.send(event)
    }
    fn clone_box(&self) -> Box<dyn RuntimeEventSender> {
        Box::new(self.clone())
    }
}
#[tokio::test]
async fn cancellation_after_content_cannot_append_into_the_next_turn() {
    let (endpoint, old_server) = fixture(vec![(
        200,
        chunk(json!({"content":"first"}), Value::Null)
            + &chunk(json!({"content":"late"}), json!("stop"))
            + "data: [DONE]\n\n",
        0,
    )])
    .await;
    let sink = Sink::default();
    let cancellation = Arc::new(crate::RunCancellation::default());
    let cancelled_sink = CancelAfterContent {
        sink: sink.clone(),
        cancellation: cancellation.clone(),
    };
    let result = run(
        &endpoint,
        None,
        "fixture",
        &[],
        5000,
        ModelStreamContext {
            reasoning_effort: "medium",
            max_output_tokens: 64,
            input: &input(),
            on_event: &cancelled_sink,
            cancellation,
            context_health: "green",
            context_sources: &[],
            context_omissions: &[],
            output_persistence: None,
        },
    )
    .await;
    assert_eq!(
        result,
        Err(Error::Cancelled {
            output_started: true
        })
    );
    old_server.await.unwrap();
    let (endpoint, new_server) = fixture(vec![(
        200,
        chunk(json!({"content":"new"}), json!("stop")) + "data: [DONE]\n\n",
        0,
    )])
    .await;
    assert_eq!(
        invoke(&endpoint, &sink, Arc::default()).await,
        Ok("new".into())
    );
    new_server.await.unwrap();
    assert_eq!(*sink.0.lock().unwrap(), ["first", "new"]);
}
