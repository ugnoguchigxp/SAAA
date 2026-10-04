use super::*;
#[path = "selector_tests/fixture.rs"]
mod fixture;
pub(super) use fixture::spawn_json_server;
use fixture::{ready_health, state_provider};
#[path = "selector_tests/contracts.rs"]
mod contracts;
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
    assert!(requests[1].to_ascii_lowercase().contains("prefer: wait=1"));
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
    let (address, server, captured_rx) =
        spawn_json_server(|_| vec![Some(saaa_selector_catalog()), Some(state.to_string()), None]);
    let result = DynamicLanConnection::resolve_at(
        Url::parse(&format!("http://{address}/")).unwrap(),
        Arc::new(RunCancellation::default()),
    )
    .await;
    assert!(
        matches!(result, Err(error) if error.code() == Some("harness-connection-schema-invalid"))
    );
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
    let (address, server, captured_rx) =
        spawn_json_server(|_| vec![Some(saaa_selector_catalog()), Some(state.to_string())]);
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
