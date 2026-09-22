#[tokio::test]
    async fn resolves_the_advertised_default_profile_through_state_and_claim() {
        let _environment = crate::test_environment::larm_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let (created_at, expires_at) = test_timestamps();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..5 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                captured_tx
                    .send(read_request(&mut stream))
                    .expect("request captured");
                let body = match index {
                    0 => json!({
                        "contractVersion": "agent-connection.v1",
                        "defaultAgentProfile": "coding-default",
                        "profiles": [{
                            "id": "coding-default",
                            "providers": [{
                                "name": "llm", "contextWindow": {"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024},
                                "capability": "llm.coding",
                                "supportedCapabilities": [
                                    "llm.coding",
                                    "llm.general",
                                    "llm.reasoning"
                                ],
                                "protocol": "openai.chat-completions.v1",
                                "model": "coding-default"
                            }]
                        }],
                        "audiences": [AUDIENCE]
                    }),
                    1 => {
                        let mut state = connection_state_json(
                            "aconn_test",
                            "ready",
                            AUDIENCE,
                            &created_at,
                            &expires_at,
                        );
                        state["agentProfile"] = json!("coding-default");
                        state["providers"][0]["capability"] = json!("llm.coding");
                        state["providers"][0]["route"] = json!("llm-default");
                        state["providers"][0]["publicModel"] = json!("coding-default");
                        state
                    }
                    2 => {
                        let mut claim =
                            claim_json("127.0.0.1", address.port(), AUDIENCE, &expires_at);
                        claim["providers"][0]["capability"] = json!("llm.coding");
                        claim["providers"][0]["model"] = json!("coding-default");
                        claim["providers"][0]["configuration"]["fields"]["model"] =
                            json!("coding-default");
                        claim
                    }
                    3 => {
                        json!({ "ready": true, "acceptingRequests": true, "capacity": {"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":2,"queueDepth":0,"queueTimeoutMs":5000,"retryAfterMs":100,"completionGuaranteed":false} })
                    }
                    4 => Value::Null,
                    _ => unreachable!(),
                };
                if index == 4 {
                    write_response(&mut stream, "204 No Content", "", "");
                } else {
                    write_response(&mut stream, "200 OK", "application/json", &body.to_string());
                }
            }
        });

        let connection = DynamicLanConnection::resolve_at(
            Url::parse(&format!("http://{address}/")).expect("control URL"),
            Arc::new(RunCancellation::default()),
        )
        .await
        .expect("current default profile resolves");
        assert_eq!(connection.model(), "coding-default");
        connection.release().await.expect("connection releases");
        server.join().expect("server joins");

        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 5);
        assert!(requests[1].contains("\"agentProfile\":\"coding-default\""));
        assert!(requests[2].starts_with("POST /v1/agent-connections/aconn_test/claim HTTP/1.1"));
        assert!(requests[4].starts_with("DELETE /v1/agent-connections/aconn_test HTTP/1.1"));

        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }
#[tokio::test]
    async fn uses_control_credential_even_when_claim_explicitly_needs_no_provider_credential() {
        let _environment = crate::test_environment::larm_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let (created_at, expires_at) = test_timestamps();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..5 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                captured_tx
                    .send(read_request(&mut stream))
                    .expect("request captured");
                let body = match index {
                    0 => json!({
                        "contractVersion": "agent-connection.v1",
                        "profiles": [{
                            "id": AGENT_PROFILE,
                            "providers": [{
                                "name": "llm", "contextWindow": {"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024},
                                "capability": PROFILE_CAPABILITY,
                                "protocol": "openai.chat-completions.v1",
                                "model": AGENT_PROFILE
                            }]
                        }],
                        "audiences": [AUDIENCE]
                    })
                    .to_string(),
                    1 => connection_state_json(
                        "aconn_test",
                        "ready",
                        AUDIENCE,
                        &created_at,
                        &expires_at,
                    )
                    .to_string(),
                    2 => anonymous_claim_json("127.0.0.1", address.port(), AUDIENCE, &expires_at)
                        .to_string(),
                    3 => json!({ "ready": true, "acceptingRequests": true, "capacity": {"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":2,"queueDepth":0,"queueTimeoutMs":5000,"retryAfterMs":100,"completionGuaranteed":false} }).to_string(),
                    4 => String::new(),
                    _ => unreachable!(),
                };
                if index == 4 {
                    write_response(&mut stream, "204 No Content", "", "");
                } else {
                    write_response(&mut stream, "200 OK", "application/json", &body);
                }
            }
        });

        let connection = DynamicLanConnection::resolve_at(
            Url::parse(&format!("http://{address}/")).expect("control URL"),
            Arc::new(RunCancellation::default()),
        )
        .await
        .expect("authenticated control connection resolves");
        assert_eq!(connection.api_key(), None);
        connection.release().await.expect("connection releases");
        server.join().expect("server joins");

        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 5);
        for index in [0, 1, 2, 4] {
            assert!(requests[index]
                .to_ascii_lowercase()
                .contains("authorization: bearer test-control-token"));
        }
        assert!(!requests[3].to_ascii_lowercase().contains("authorization:"));

        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }
#[tokio::test]
    async fn releases_when_semantic_health_is_not_ready() {
        let _environment = crate::test_environment::larm_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let (created_at, expires_at) = test_timestamps();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..5 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                let request = read_request(&mut stream);
                captured_tx.send(request).expect("request captured");
                let body = match index {
                    0 => json!({
                        "contractVersion": "agent-connection.v1",
                        "profiles": [{
                            "id": AGENT_PROFILE,
                            "providers": [{
                                "name": "llm", "contextWindow": {"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024},
                                "capability": PROFILE_CAPABILITY,
                                "protocol": "openai.chat-completions.v1",
                                "model": AGENT_PROFILE
                            }]
                        }],
                        "audiences": [AUDIENCE]
                    })
                    .to_string(),
                    1 => connection_state_json(
                        "aconn_test",
                        "ready",
                        AUDIENCE,
                        &created_at,
                        &expires_at,
                    )
                    .to_string(),
                    2 => claim_json("127.0.0.1", address.port(), AUDIENCE, &expires_at).to_string(),
                    3 => json!({ "ready": true, "acceptingRequests": false, "capacity": {"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":2,"queueDepth":0,"queueTimeoutMs":5000,"retryAfterMs":100,"completionGuaranteed":false} }).to_string(),
                    4 => String::new(),
                    _ => unreachable!(),
                };
                if index == 4 {
                    write_response(&mut stream, "204 No Content", "", "");
                } else {
                    write_response(&mut stream, "200 OK", "application/json", &body);
                }
            }
        });

        let error = match DynamicLanConnection::resolve_at(
            Url::parse(&format!("http://{address}/")).expect("control URL"),
            Arc::new(RunCancellation::default()),
        )
        .await
        {
            Ok(_) => panic!("semantic health failure must reject the connection"),
            Err(error) => error,
        };
        assert_eq!(error.kind, ErrorKind::Unavailable);
        server.join().expect("server joins");
        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 5);
        assert!(requests[4].starts_with("DELETE /v1/agent-connections/aconn_test HTTP/1.1"));

        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }
#[tokio::test]
    async fn renews_and_reclaims_before_the_request_deadline() {
        let _environment = crate::test_environment::larm_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let created_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
        let initial_expires_at = (chrono::Utc::now() + chrono::Duration::seconds(45)).to_rfc3339();
        let renewed_expires_at = (chrono::Utc::now() + chrono::Duration::seconds(299)).to_rfc3339();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..8 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                let request = read_request(&mut stream);
                captured_tx.send(request).expect("request captured");
                let body = match index {
                    0 => json!({
                        "contractVersion": "agent-connection.v1",
                        "profiles": [{
                            "id": AGENT_PROFILE,
                            "providers": [{
                                "name": "llm", "contextWindow": {"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024},
                                "capability": PROFILE_CAPABILITY,
                                "protocol": "openai.chat-completions.v1",
                                "model": AGENT_PROFILE
                            }]
                        }],
                        "audiences": [AUDIENCE]
                    })
                    .to_string(),
                    1 => connection_state_json(
                        "aconn_test",
                        "ready",
                        AUDIENCE,
                        &created_at,
                        &initial_expires_at,
                    )
                    .to_string(),
                    2 => claim_json("127.0.0.1", address.port(), AUDIENCE, &initial_expires_at)
                        .to_string(),
                    3 | 6 => json!({ "ready": true, "acceptingRequests": true, "capacity": {"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":2,"queueDepth":0,"queueTimeoutMs":5000,"retryAfterMs":100,"completionGuaranteed":false} }).to_string(),
                    4 => connection_state_json(
                        "aconn_test",
                        "ready",
                        AUDIENCE,
                        &created_at,
                        &renewed_expires_at,
                    )
                    .to_string(),
                    5 => claim_json("127.0.0.1", address.port(), AUDIENCE, &renewed_expires_at)
                        .to_string(),
                    7 => String::new(),
                    _ => unreachable!(),
                };
                if index == 7 {
                    write_response(&mut stream, "204 No Content", "", "");
                } else {
                    write_response(&mut stream, "200 OK", "application/json", &body);
                }
            }
        });

        let mut connection = DynamicLanConnection::resolve_at(
            Url::parse(&format!("http://{address}/")).expect("control URL"),
            Arc::new(RunCancellation::default()),
        )
        .await
        .expect("connection resolves");
        connection
            .ensure_lifetime(
                Duration::from_secs(30),
                Arc::new(RunCancellation::default()),
            )
            .await
            .expect("connection renews and reclaims");
        connection.release().await.expect("connection releases");
        server.join().expect("server joins");

        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 8);
        assert!(requests[4].starts_with("POST /v1/agent-connections/aconn_test/renew HTTP/1.1"));
        assert!(requests[4]
            .to_ascii_lowercase()
            .contains("idempotency-key:"));
        assert!(requests[5].starts_with("POST /v1/agent-connections/aconn_test/claim HTTP/1.1"));
        assert!(!requests[5]
            .to_ascii_lowercase()
            .contains("idempotency-key:"));
        assert!(requests[6]
            .starts_with("GET /v1/agent-connections/aconn_test/providers/llm/health HTTP/1.1"));
        assert!(requests[7].starts_with("DELETE /v1/agent-connections/aconn_test HTTP/1.1"));

        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }
#[cfg(not(coverage))]
    #[tokio::test]
    #[ignore = "operator-only live dynamic_lan Agent Connection API canary"]
    async fn live_dynamic_lan_claim_and_chat() {
        let _environment = crate::test_environment::larm_lock().lock().await;
        let host = env::var("SAAA_DYNAMIC_LAN_HOST").expect("SAAA_DYNAMIC_LAN_HOST is required");
        let connection = DynamicLanConnection::resolve(&host, Arc::new(RunCancellation::default()))
            .await
            .expect("live dynamic_lan connection resolves");
        let authorization = connection
            .api_key()
            .map(|credential| format!("Bearer {credential}"));
        let prompt = "Reply with exactly: SAAA_DYNAMIC_OK";
        let input = crate::StartTurnInput {
            run_id: format!("run_dynamic_canary_{}", Uuid::new_v4().simple()),
            conversation_id: "conversation_dynamic_canary".to_string(),
            content: prompt.to_string(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".to_string(),
            presentation_mode: "visual".to_string(),
        };
        let events = LiveCanaryEvents;
        let history = [crate::ipc_contract::ConversationMessage {
            parts: None,
            id: "probe".into(),
            conversation_id: input.conversation_id.clone(),
            role: "user".into(),
            content: prompt.into(),
            created_at: String::new(),
        }];
        let result = crate::providers::chat_completions::run(
            connection.endpoint(),
            authorization.as_deref(),
            connection.model(),
            &history,
            120_000,
            crate::providers::stream::ModelStreamContext {
                reasoning_effort: "low",
                max_output_tokens: 512,
                input: &input,
                on_event: &events,
                cancellation: Arc::default(),
                context_health: "green",
                context_sources: &[],
                context_omissions: &[],
                output_persistence: None,
            },
        )
        .await;
        let release = connection.release().await;
        let completion = result.expect("claimed HTTP provider request completes");
        assert!(!completion.trim().is_empty());
        release.expect("live connection releases");
    }
#[derive(Clone, Copy)]
    struct LiveCanaryEvents;
impl crate::runtime::event_hub::RuntimeEventSender for LiveCanaryEvents {
        fn send(&self, _event: crate::ipc_contract::RuntimeEvent) -> tauri::Result<()> {
            Ok(())
        }

        fn clone_box(&self) -> Box<dyn crate::runtime::event_hub::RuntimeEventSender> {
            Box::new(*self)
        }
    }
fn read_request(stream: &mut std::net::TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected = None;
        loop {
            let read = stream.read(&mut buffer).expect("request reads");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected.is_none() {
                if let Some(header_end) =
                    request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    expected = Some(header_end + 4 + content_length);
                }
            }
            if expected.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        String::from_utf8(request).expect("request is UTF-8")
    }
fn write_response(
        stream: &mut std::net::TcpStream,
        status: &str,
        content_type: &str,
        body: &str,
    ) {
        write_response_with_headers(stream, status, content_type, "", body);
    }
fn write_response_with_headers(
        stream: &mut std::net::TcpStream,
        status: &str,
        content_type: &str,
        extra_headers: &str,
        body: &str,
    ) {
        let content_type = if content_type.is_empty() {
            String::new()
        } else {
            format!("Content-Type: {content_type}\r\n")
        };
        let revision = if content_type.is_empty() {
            String::new()
        } else {
            format!("x-larm-config-revision: {TEST_REVISION}\r\n")
        };
        write!(
            stream,
            "HTTP/1.1 {status}\r\n{content_type}{revision}{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("response writes");
    }
