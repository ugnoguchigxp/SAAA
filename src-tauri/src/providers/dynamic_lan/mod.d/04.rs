#[tokio::test]
    async fn error_status_is_classified_before_success_only_retry_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let _ = read_request(&mut stream);
            write_response_with_headers(
                &mut stream,
                "429 Too Many Requests",
                "application/json",
                "Retry-After: 120\r\nLocation: invalid location\r\n",
                &json!({ "error": { "code": "capacity_exhausted" } }).to_string(),
            );
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client");
        let error = match send_json_response::<Value>(
            &client,
            Method::GET,
            Url::parse(&format!("http://{address}/v3/agent-profiles")).expect("test URL"),
            Some(&provider_credential("test-control-token").expect("test credential")),
            None,
            None,
            &RunCancellation::default(),
        )
        .await
        {
            Ok(_) => panic!("429 must fail"),
            Err(error) => error,
        };
        server.join().expect("server joins");
        assert_eq!(error.kind, ErrorKind::Capacity);
    }
#[tokio::test]
    async fn initialization_error_preserves_a_release_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let request = read_request(&mut stream);
            assert!(request.starts_with("DELETE "));
            write_response(
                &mut stream,
                "503 Service Unavailable",
                "application/json",
                "{}",
            );
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test client");
        let error = error_after_release(
            contract_error(()),
            &client,
            &Url::parse(&format!("http://{address}/v1/agent-connections/aconn_test"))
                .expect("release URL"),
            Some(&provider_credential("test-control-token").expect("test credential")),
        )
        .await;
        server.join().expect("server joins");
        assert_eq!(error.kind, ErrorKind::Contract);
        assert_eq!(error.release_failure(), Some(ErrorKind::Unavailable));
    }
#[tokio::test]
    async fn releases_the_original_connection_when_poll_identity_changes() {
        let _environment = crate::test_environment::larm_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let (created_at, expires_at) = test_timestamps();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..4 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                let request = read_request(&mut stream);
                captured_tx.send(request).expect("request captured");
                match index {
                    0 => write_response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &json!({
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
                    ),
                    1 => write_response_with_headers(
                        &mut stream,
                        "202 Accepted",
                        "application/json",
                        "Location: /v1/agent-connections/aconn_original\r\nRetry-After: 1\r\n",
                        &connection_state_json(
                            "aconn_original",
                            "pending",
                            AUDIENCE,
                            &created_at,
                            &expires_at,
                        )
                        .to_string(),
                    ),
                    2 => write_response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &connection_state_json(
                            "aconn_changed",
                            "ready",
                            AUDIENCE,
                            &created_at,
                            &expires_at,
                        )
                        .to_string(),
                    ),
                    3 => write_response(&mut stream, "204 No Content", "", ""),
                    _ => unreachable!(),
                }
            }
        });

        let error = match DynamicLanConnection::resolve_at(
            Url::parse(&format!("http://{address}/")).expect("control URL"),
            Arc::new(RunCancellation::default()),
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("changed connection identity must be rejected"),
        };
        assert_eq!(error.kind, ErrorKind::Contract);
        server.join().expect("server joins");
        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 4);
        assert!(requests[2].starts_with("GET /v1/agent-connections/aconn_original HTTP/1.1"));
        assert!(requests[3].starts_with("DELETE /v1/agent-connections/aconn_original HTTP/1.1"));

        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }
#[tokio::test]
    async fn resolves_claimed_openai_settings_and_releases_the_connection() {
        let _environment = crate::test_environment::larm_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let (created_at, expires_at) = test_timestamps();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..6 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                let request = read_request(&mut stream);
                captured_tx.send(request.clone()).expect("request captured");
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
                    2 => {
                        let mut claim =
                            claim_json("127.0.0.1", address.port(), AUDIENCE, &expires_at);
                        claim["providers"][0]
                            .as_object_mut()
                            .unwrap()
                            .remove("streaming");
                        claim.to_string()
                    }
                    3 => json!({ "ready": true, "acceptingRequests": true, "capacity": {"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":2,"queueDepth":0,"queueTimeoutMs":5000,"retryAfterMs":100,"completionGuaranteed":false} }).to_string(),
                    4 => format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        json!({"model": AGENT_PROFILE, "choices":[{"index":0,"delta":{"content":"HTTP claim works"},"finish_reason":"stop"}]})
                    ),
                    5 => String::new(),
                    _ => unreachable!(),
                };
                if index == 5 {
                    write_response(&mut stream, "204 No Content", "", "");
                } else if index == 4 {
                    write_response(&mut stream, "200 OK", "text/event-stream", &body);
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
        .expect("connection resolves");
        assert_eq!(
            connection.endpoint(),
            format!("http://127.0.0.1:{}/v1", address.port())
        );
        assert_eq!(connection.model(), AGENT_PROFILE);
        assert_eq!(connection.api_key(), Some("short-lived-provider-token"));
        assert_eq!(connection.capacity(), (1, 0, 2, 0, 5_000, 100, false));
        connection.validate_request_budget(4_096).unwrap();
        assert!(connection.validate_request_budget(4_097).is_err());
        let input = crate::StartTurnInput {
            run_id: "claim-fixture".into(),
            conversation_id: "claim-fixture".into(),
            content: "test".into(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        };
        let authorization = format!("Bearer {}", connection.api_key().unwrap());
        let result = crate::providers::chat_completions::run(
            connection.endpoint(),
            Some(&authorization),
            connection.model(),
            &[],
            5000,
            crate::providers::stream::ModelStreamContext {
                reasoning_effort: "low",
                max_output_tokens: 64,
                input: &input,
                on_event: &LiveCanaryEvents,
                cancellation: Arc::default(),
                context_health: "green",
                context_sources: &[],
                context_omissions: &[],
                output_persistence: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "HTTP claim works");
        connection.release().await.expect("connection releases");
        server.join().expect("server joins");

        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 6);
        assert!(requests[0].starts_with("GET /v3/agent-profiles?profile=SAAA HTTP/1.1"));
        assert!(requests[1].starts_with("POST /v1/agent-connections HTTP/1.1"));
        assert!(requests[1].contains("\"profile\":\"SAAA\""));
        assert!(requests[1].contains("\"ttlSeconds\":300"));
        assert!(requests[2].starts_with("POST /v1/agent-connections/aconn_test/claim HTTP/1.1"));
        assert!(!requests[2]
            .to_ascii_lowercase()
            .contains("idempotency-key:"));
        assert!(requests[3]
            .starts_with("GET /v1/agent-connections/aconn_test/providers/llm/health HTTP/1.1"));
        assert!(requests[5].starts_with("DELETE /v1/agent-connections/aconn_test HTTP/1.1"));
        for index in [0, 1, 2, 5] {
            assert!(requests[index]
                .to_ascii_lowercase()
                .contains("authorization: bearer test-control-token"));
        }
        assert!(requests[3]
            .to_ascii_lowercase()
            .contains("authorization: bearer short-lived-provider-token"));

        assert!(requests[4].starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(requests[4]
            .to_ascii_lowercase()
            .contains("authorization: bearer short-lived-provider-token"));
        assert!(!requests[4].contains("test-control-token"));
        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }
#[tokio::test]
    async fn wr_t13_real_allocation_refreshes_source_frame_and_releases() {
        let _environment = crate::test_environment::larm_lock().lock().await;
        let previous_token = env::var(API_TOKEN_ENV).ok();
        env::set_var(API_TOKEN_ENV, "test-control-token");

        let harness = crate::runtime::context::world::wire_test_support::Harness::new();
        let clock = harness.fixture.clock.clone();
        let before = harness.fixture.now();
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
        let address = listener.local_addr().expect("listener address");
        let (created_at, expires_at) = test_timestamps();
        let (captured_tx, captured_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for index in 0..6 {
                let (mut stream, _) = listener.accept().expect("request accepted");
                let request = read_request(&mut stream);
                captured_tx.send(request.clone()).expect("request captured");
                if index == 1 {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    clock.store(before + 3000, std::sync::atomic::Ordering::SeqCst);
                }
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
                    2 => {
                        let mut claim =
                            claim_json("127.0.0.1", address.port(), AUDIENCE, &expires_at);
                        claim["providers"][0]
                            .as_object_mut()
                            .unwrap()
                            .remove("streaming");
                        claim.to_string()
                    }
                    3 => json!({ "ready": true, "acceptingRequests": true, "capacity": {"maxConcurrentRequests":1,"activeRequests":0,"maxQueuedRequests":2,"queueDepth":0,"queueTimeoutMs":5000,"retryAfterMs":100,"completionGuaranteed":false} }).to_string(),
                    4 => format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        json!({"model": AGENT_PROFILE, "choices":[{"index":0,"delta":{"content":"HTTP claim works"},"finish_reason":"stop"}]})
                    ),
                    5 => String::new(),
                    _ => unreachable!(),
                };
                if index == 5 {
                    write_response(&mut stream, "204 No Content", "", "");
                } else if index == 4 {
                    write_response(&mut stream, "200 OK", "text/event-stream", &body);
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
        .expect("connection resolves");
        assert_eq!(
            connection.endpoint(),
            format!("http://127.0.0.1:{}/v1", address.port())
        );
        assert_eq!(connection.model(), AGENT_PROFILE);
        assert_eq!(connection.api_key(), Some("short-lived-provider-token"));
        let input = crate::StartTurnInput {
            run_id: crate::memory::personal_state::world::runtime_test_support::RUN_ID.into(),
            conversation_id: crate::PRIMARY_CONVERSATION_ID.into(),
            content: "hello".into(),
            workspace_path: None,
            retry_input_message_id: None,
            source_id: None,
            scope_refs: Vec::new(),
            input_origin: "text".into(),
            presentation_mode: "visual".into(),
        };
        let authorization = format!("Bearer {}", connection.api_key().unwrap());
        let result = crate::providers::chat_completions::run(
            connection.endpoint(),
            Some(&authorization),
            connection.model(),
            &harness.history,
            5000,
            crate::providers::stream::ModelStreamContext {
                reasoning_effort: "low",
                max_output_tokens: 64,
                input: &input,
                on_event: &LiveCanaryEvents,
                cancellation: Arc::default(),
                context_health: "green",
                context_sources: &harness.composed.envelope.selected,
                context_omissions: &harness.composed.envelope.omitted,
                output_persistence: Some(crate::ProviderOutputPersistence {
                    state: &harness.state,
                    session_id: &harness.session,
                    world: harness.composed.world.as_ref(),
                }),
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "HTTP claim works");
        connection.release().await.expect("connection releases");
        server.join().expect("server joins");

        let requests = captured_rx.try_iter().collect::<Vec<_>>();
        assert_eq!(requests.len(), 6);
        let body: Value =
            serde_json::from_str(requests[4].split_once("\r\n\r\n").unwrap().1).unwrap();
        harness.assert_wire(&body);
        assert!(requests[0].starts_with("GET /v3/agent-profiles?profile=SAAA HTTP/1.1"));
        assert!(requests[1].starts_with("POST /v1/agent-connections HTTP/1.1"));
        assert!(requests[1].contains("\"profile\":\"SAAA\""));
        assert!(requests[1].contains("\"ttlSeconds\":300"));
        assert!(requests[2].starts_with("POST /v1/agent-connections/aconn_test/claim HTTP/1.1"));
        assert!(!requests[2]
            .to_ascii_lowercase()
            .contains("idempotency-key:"));
        assert!(requests[3]
            .starts_with("GET /v1/agent-connections/aconn_test/providers/llm/health HTTP/1.1"));
        assert!(requests[5].starts_with("DELETE /v1/agent-connections/aconn_test HTTP/1.1"));
        for index in [0, 1, 2, 5] {
            assert!(requests[index]
                .to_ascii_lowercase()
                .contains("authorization: bearer test-control-token"));
        }
        assert!(requests[3]
            .to_ascii_lowercase()
            .contains("authorization: bearer short-lived-provider-token"));

        assert!(requests[4].starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(requests[4]
            .to_ascii_lowercase()
            .contains("authorization: bearer short-lived-provider-token"));
        assert!(!requests[4].contains("test-control-token"));
        if let Some(token) = previous_token {
            env::set_var(API_TOKEN_ENV, token);
        } else {
            env::remove_var(API_TOKEN_ENV);
        }
    }
