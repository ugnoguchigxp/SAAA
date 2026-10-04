use super::*;
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
