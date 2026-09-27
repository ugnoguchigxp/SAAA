fn state_provider(name: &str) -> Value {
    let (protocol, endpoint, model) = match name {
        "llm" => (
            "openai.chat-completions.v1",
            "/v1/chat/completions",
            LLM_MODEL,
        ),
        "backchannel" => (
            "openai.chat-completions.v1",
            "/v1/chat/completions",
            "qwen3.5-2b-fast-response",
        ),
        "asr" => (
            "openai.audio-transcriptions.v1",
            "/v1/audio/transcriptions",
            "qwen3-asr-1.7b",
        ),
        "tts" => (
            "openai.audio-speech.v1",
            "/v1/audio/speech",
            "voicevox-core",
        ),
        "embedding" => ("larm.embedding.v1", "/v1/embed", "multilingual-e5-small"),
        _ => ("invalid", "/invalid", "invalid"),
    };
    json!({
        "name": name,
        "capability": if name == "llm" { PROFILE_CAPABILITY } else { "ignored" },
        "route": "route-a",
        "protocol": protocol,
        "endpoint": endpoint,
        "model": model,
        "readiness": "ready",
        "claimable": true
    })
}

fn spawn_json_server(
    build: impl FnOnce(u16) -> Vec<Option<String>>,
) -> (
    std::net::SocketAddr,
    std::thread::JoinHandle<()>,
    mpsc::Receiver<String>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("test listener");
    let address = listener.local_addr().expect("listener address");
    let responses = build(address.port());
    let (captured_tx, captured_rx) = mpsc::channel();
    let server = std::thread::spawn(move || {
        for (index, response) in responses.into_iter().enumerate() {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let request = read_request(&mut stream);
            captured_tx.send(request).expect("request captured");
            match response {
                Some(body) => write_response(
                    &mut stream,
                    if index == 1 { "201 Created" } else { "200 OK" },
                    "application/json",
                    &body,
                ),
                None => write_response(&mut stream, "204 No Content", "", ""),
            }
        }
    });
    (address, server, captured_rx)
}

fn ready_health() -> String {
    json!({
        "ready": true,
        "acceptingRequests": true,
        "capacity": {
            "maxConcurrentRequests": 1,
            "activeRequests": 0,
            "maxQueuedRequests": 2,
            "queueDepth": 0,
            "queueTimeoutMs": 5000,
            "retryAfterMs": 100,
            "completionGuaranteed": false
        }
    })
    .to_string()
}

#[tokio::test]
async fn text_path_queries_saaa_selector() {
    let _environment = crate::test_environment::larm_lock().lock().await;
    let previous_token = env::var(API_TOKEN_ENV).ok();
    env::set_var(API_TOKEN_ENV, "test-control-token");
    let (created_at, expires_at) = test_timestamps();
    let (address, server, captured_rx) = spawn_json_server(|port| {
        vec![
            Some(saaa_selector_catalog()),
            Some(
                connection_state_json("aconn_test", "ready", AUDIENCE, &created_at, &expires_at)
                    .to_string(),
            ),
            Some(claim_json("127.0.0.1", port, AUDIENCE, &expires_at).to_string()),
            Some(ready_health()),
            None,
        ]
    });
    let connection = DynamicLanConnection::resolve_at(
        Url::parse(&format!("http://{address}/")).expect("control URL"),
        Arc::new(RunCancellation::default()),
    )
    .await
    .expect("selector resolves");
    connection.release().await.expect("connection releases");
    server.join().expect("server joins");
    let requests = captured_rx.try_iter().collect::<Vec<_>>();
    assert!(requests[0].contains("GET /v3/agent-profiles?profile=SAAA"));
    assert!(requests[1].contains("\"profile\":\"SAAA\""));
    assert!(!requests[1].contains("explicitAgentProfile"));
    assert!(!requests[1].contains("\"agentProfile\":"));
    assert!(requests[1]
        .to_ascii_lowercase()
        .contains("prefer: wait=1"));
    assert!(requests[1].contains("\"expectedCatalogRevision\":\"rev-fixture\""));
    if let Some(token) = previous_token {
        env::set_var(API_TOKEN_ENV, token);
    } else {
        env::remove_var(API_TOKEN_ENV);
    }
}

#[tokio::test]
async fn text_path_accepts_five_provider_state_in_any_order() {
    let _environment = crate::test_environment::larm_lock().lock().await;
    let previous_token = env::var(API_TOKEN_ENV).ok();
    env::set_var(API_TOKEN_ENV, "test-control-token");
    let (created_at, expires_at) = test_timestamps();
    let mut state =
        connection_state_json("aconn_test", "ready", AUDIENCE, &created_at, &expires_at);
    state["providers"] = json!([
        state_provider("tts"),
        state_provider("llm"),
        state_provider("embedding"),
        state_provider("backchannel"),
        state_provider("asr")
    ]);
    let (address, server, _captured_rx) = spawn_json_server(|port| {
        vec![
            Some(saaa_selector_catalog()),
            Some(state.to_string()),
            Some(claim_json("127.0.0.1", port, AUDIENCE, &expires_at).to_string()),
            Some(ready_health()),
            None,
        ]
    });
    let connection = DynamicLanConnection::resolve_at(
        Url::parse(&format!("http://{address}/")).expect("control URL"),
        Arc::new(RunCancellation::default()),
    )
    .await
    .expect("five providers resolve");
    assert_eq!(connection.model(), "ornith-1.5-35b");
    connection.release().await.expect("connection releases");
    server.join().expect("server joins");
    if let Some(token) = previous_token {
        env::set_var(API_TOKEN_ENV, token);
    } else {
        env::remove_var(API_TOKEN_ENV);
    }
}

#[tokio::test]
async fn text_path_accepts_live_connection_state_without_routes() {
    let _environment = crate::test_environment::larm_lock().lock().await;
    let previous_token = env::var(API_TOKEN_ENV).ok();
    env::set_var(API_TOKEN_ENV, "test-control-token");
    let (created_at, expires_at) = test_timestamps();
    let mut state =
        connection_state_json("aconn_test", "ready", AUDIENCE, &created_at, &expires_at);
    for provider in state["providers"].as_array_mut().unwrap() {
        provider.as_object_mut().unwrap().remove("route");
    }
    let (address, server, captured_rx) = spawn_json_server(|port| {
        vec![
            Some(saaa_selector_catalog()),
            Some(state.to_string()),
            Some(claim_json("127.0.0.1", port, AUDIENCE, &expires_at).to_string()),
            Some(ready_health()),
            None,
        ]
    });
    let connection = DynamicLanConnection::resolve_at(
        Url::parse(&format!("http://{address}/")).unwrap(),
        Arc::new(RunCancellation::default()),
    )
    .await
    .expect("live connection state resolves");
    connection.release().await.expect("connection releases");
    server.join().expect("server joins");
    assert_eq!(
        captured_rx
            .try_iter()
            .filter(|request| request.starts_with("DELETE "))
            .count(),
        1
    );
    if let Some(token) = previous_token {
        env::set_var(API_TOKEN_ENV, token);
    } else {
        env::remove_var(API_TOKEN_ENV);
    }
}

#[tokio::test]
async fn malformed_create_state_releases_connection_id() {
    let _environment = crate::test_environment::larm_lock().lock().await;
    let previous_token = env::var(API_TOKEN_ENV).ok();
    env::set_var(API_TOKEN_ENV, "test-control-token");
    let (created_at, expires_at) = test_timestamps();
    let mut state =
        connection_state_json("aconn_test", "ready", AUDIENCE, &created_at, &expires_at);
    state.as_object_mut().unwrap().remove("profileRevision");
    let (address, server, captured_rx) = spawn_json_server(|_| {
        vec![Some(saaa_selector_catalog()), Some(state.to_string()), None]
    });
    let result = DynamicLanConnection::resolve_at(
        Url::parse(&format!("http://{address}/")).unwrap(),
        Arc::new(RunCancellation::default()),
    )
    .await;
    assert!(matches!(result, Err(error) if error.code() == Some("harness-connection-schema-invalid")));
    server.join().expect("server joins");
    assert_eq!(
        captured_rx
            .try_iter()
            .filter(|request| request.starts_with("DELETE /v1/agent-connections/aconn_test"))
            .count(),
        1
    );
    if let Some(token) = previous_token {
        env::set_var(API_TOKEN_ENV, token);
    } else {
        env::remove_var(API_TOKEN_ENV);
    }
}

#[tokio::test]
async fn create_without_an_identifier_reports_deferred_cleanup() {
    let _environment = crate::test_environment::larm_lock().lock().await;
    let previous_token = env::var(API_TOKEN_ENV).ok();
    env::set_var(API_TOKEN_ENV, "test-control-token");
    let (created_at, expires_at) = test_timestamps();
    let state = connection_state_json("", "ready", AUDIENCE, &created_at, &expires_at);
    let (address, server, captured_rx) = spawn_json_server(|_| {
        vec![Some(saaa_selector_catalog()), Some(state.to_string())]
    });
    let result = DynamicLanConnection::resolve_at(
        Url::parse(&format!("http://{address}/")).unwrap(),
        Arc::new(RunCancellation::default()),
    )
    .await;
    assert!(matches!(result, Err(error) if error.release_failure() == Some(ErrorKind::Contract)));
    server.join().expect("server joins");
    assert_eq!(
        captured_rx
            .try_iter()
            .filter(|request| request.starts_with("DELETE "))
            .count(),
        0
    );
    if let Some(token) = previous_token {
        env::set_var(API_TOKEN_ENV, token);
    } else {
        env::remove_var(API_TOKEN_ENV);
    }
}

#[tokio::test]
async fn text_path_rejects_duplicate_llm() {
    let _environment = crate::test_environment::larm_lock().lock().await;
    let previous_token = env::var(API_TOKEN_ENV).ok();
    env::set_var(API_TOKEN_ENV, "test-control-token");
    let (created_at, expires_at) = test_timestamps();
    let mut state =
        connection_state_json("aconn_test", "ready", AUDIENCE, &created_at, &expires_at);
    state["providers"] = json!([state_provider("llm"), state_provider("llm")]);
    let (address, server, captured_rx) = spawn_json_server(|_port| {
        vec![Some(saaa_selector_catalog()), Some(state.to_string()), None]
    });
    let error = match DynamicLanConnection::resolve_at(
        Url::parse(&format!("http://{address}/")).expect("control URL"),
        Arc::new(RunCancellation::default()),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("duplicate llm is a contract error"),
    };
    assert_eq!(error.kind, ErrorKind::Contract);
    server.join().expect("server joins");
    let releases = captured_rx
        .try_iter()
        .filter(|request| request.starts_with("DELETE "))
        .count();
    assert_eq!(releases, 1);
    if let Some(token) = previous_token {
        env::set_var(API_TOKEN_ENV, token);
    } else {
        env::remove_var(API_TOKEN_ENV);
    }
}

#[tokio::test]
async fn text_path_rejects_claim_model_mismatch() {
    let _environment = crate::test_environment::larm_lock().lock().await;
    let previous_token = env::var(API_TOKEN_ENV).ok();
    env::set_var(API_TOKEN_ENV, "test-control-token");
    let (created_at, expires_at) = test_timestamps();
    let (address, server, captured_rx) = spawn_json_server(|port| {
        let mut claim = claim_json("127.0.0.1", port, AUDIENCE, &expires_at);
        provider_mut(&mut claim, "llm")["model"] = json!("not-the-catalog-model");
        vec![
            Some(saaa_selector_catalog()),
            Some(
                connection_state_json("aconn_test", "ready", AUDIENCE, &created_at, &expires_at)
                    .to_string(),
            ),
            Some(claim.to_string()),
            None,
        ]
    });
    let error = match DynamicLanConnection::resolve_at(
        Url::parse(&format!("http://{address}/")).expect("control URL"),
        Arc::new(RunCancellation::default()),
    )
    .await
    {
        Err(error) => error,
        Ok(_) => panic!("claim model mismatch is a contract error"),
    };
    assert_eq!(error.kind, ErrorKind::Contract);
    server.join().expect("server joins");
    let releases = captured_rx
        .try_iter()
        .filter(|request| request.starts_with("DELETE "))
        .count();
    assert_eq!(releases, 1);
    if let Some(token) = previous_token {
        env::set_var(API_TOKEN_ENV, token);
    } else {
        env::remove_var(API_TOKEN_ENV);
    }
}
