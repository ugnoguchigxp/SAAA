use super::*;
use serde_json::Value;
use std::env;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
const PROFILE_ID: &str = "saaa-conversation-ornith15";
const LLM_MODEL: &str = "ornith-1.5-35b";
const PROFILE_CAPABILITY: &str = "llm.general";
const TEST_REVISION: &str = "0000000000000000000000000000000000000000000000000000000000000000";
#[test]
fn create_errors_keep_distinct_contract_codes() {
    assert_eq!(
        classify_status(StatusCode::CONFLICT, "catalog_revision_mismatch").code(),
        Some("larm_revision_mismatch")
    );
    assert_eq!(
        classify_status(StatusCode::CONFLICT, "idempotency_conflict").code(),
        Some("larm_idempotency_conflict")
    );
    assert_eq!(
        classify_status(StatusCode::CONFLICT, "connection_audience_unavailable").code(),
        Some("larm_provider_conflict")
    );
    assert_eq!(
        classify_status(StatusCode::BAD_REQUEST, "unknown_selector").code(),
        Some("larm_unknown_selector")
    );
    assert_eq!(
        classify_status(StatusCode::SERVICE_UNAVAILABLE, "provider_terminal").code(),
        Some("larm_provider_terminal")
    );
}
#[test]
fn catalog_model_mismatch_in_non_llm_provider_is_rejected() {
    let (created_at, expires_at) = test_timestamps();
    let mut identity = test_identity("aconn_test", &created_at, &expires_at);
    let mut state =
        connection_state_json("aconn_test", "ready", AUDIENCE, &created_at, &expires_at);
    identity.profile.catalog_models = Some(
        state["providers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                (
                    entry["name"].as_str().unwrap().to_string(),
                    entry["model"].as_str().unwrap().to_string(),
                )
            })
            .collect(),
    );
    state["providers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["name"] == "backchannel")
        .unwrap()["model"] = json!("unexpected-model");
    let state: ConnectionState = serde_json::from_value(state).unwrap();
    assert!(validate_state_shape(&state, AUDIENCE, &identity.profile).is_err());
}
fn provider_mut<'a>(value: &'a mut Value, name: &str) -> &'a mut Value {
    value["providers"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|provider| provider["name"] == name)
        .unwrap()
}
fn saaa_selector_catalog() -> String {
    json!({
        "contractVersion": "agent-connection.v3",
        "catalogRevision": "rev-fixture",
        "requestedProfile": "SAAA",
        "profiles": [{
            "id": PROFILE_ID,
            "providers": [
                {"name":"llm","capability":PROFILE_CAPABILITY,"protocol":"openai.chat-completions.v1","endpoint":"/v1/chat/completions","model":LLM_MODEL,"contextWindow":{"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024}},
                {"name":"backchannel","capability":"llm.backchannel.classifier","protocol":"openai.chat-completions.v1","endpoint":"/v1/chat/completions","model":"qwen3.5-2b-fast-response","contextWindow":{"maxTokens":65536,"outputReserveTokens":4096,"safetyMarginTokens":1976}},
                {"name":"asr","capability":"speech.stt","protocol":"openai.audio-transcriptions.v1","endpoint":"/v1/audio/transcriptions","model":"qwen3-asr-1.7b"},
                {"name":"tts","capability":"speech.tts","protocol":"openai.audio-speech.v1","endpoint":"/v1/audio/speech","model":"voicevox-core"},
                {"name":"embedding","capability":"embedding","protocol":"larm.embedding.v1","endpoint":"/v1/embed","model":"multilingual-e5-small"}
            ],
            "services": []
        }]
    }).to_string()
}
fn test_timestamps() -> (String, String) {
    let created = chrono::Utc::now() - chrono::Duration::seconds(1);
    let expires = created + chrono::Duration::seconds(CONNECTION_TTL_SECONDS.into());
    (created.to_rfc3339(), expires.to_rfc3339())
}
fn connection_state_json(
    id: &str,
    status: &str,
    audience: &str,
    created_at: &str,
    expires_at: &str,
) -> Value {
    let readiness = if status == "ready" { "ready" } else { status };
    json!({
        "id": id,
        "allocationId": "alloc_test",
        "bootEpoch": "epoch_test",
        "catalogRevision": TEST_REVISION,
        "profile": "SAAA",
        "agentProfile": PROFILE_ID,
        "profileRevision": TEST_REVISION,
        "audience": audience,
        "audienceRevision": TEST_REVISION,
        "status": status,
        "services": [],
        "providers": [{
            "name": "llm", "contextWindow": {"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024},
            "capability": PROFILE_CAPABILITY,
            "route": "llm-agent-35b",
            "protocol": "openai.chat-completions.v1",
            "endpoint": "/v1/chat/completions",
            "model": LLM_MODEL,
            "readiness": readiness,
            "claimable": status == "ready"
        },
        {"name":"backchannel","capability":"llm.backchannel.classifier","route":"backchannel","protocol":"openai.chat-completions.v1","endpoint":"/v1/chat/completions","model":"qwen3.5-2b-fast-response","readiness":readiness,"claimable":status == "ready"},
        {"name":"asr","capability":"speech.stt","route":"asr","protocol":"openai.audio-transcriptions.v1","endpoint":"/v1/audio/transcriptions","model":"qwen3-asr-1.7b","readiness":readiness,"claimable":status == "ready"},
        {"name":"tts","capability":"speech.tts","route":"tts","protocol":"openai.audio-speech.v1","endpoint":"/v1/audio/speech","model":"voicevox-core","readiness":readiness,"claimable":status == "ready"},
        {"name":"embedding","capability":"embedding","route":"embedding","protocol":"larm.embedding.v1","endpoint":"/v1/embed","model":"multilingual-e5-small","readiness":readiness,"claimable":status == "ready"}],
        "createdAt": created_at,
        "expiresAt": expires_at,
        "error": null
    })
}
fn test_identity(id: &str, created_at: &str, expires_at: &str) -> ConnectionIdentity {
    ConnectionIdentity {
        id: id.to_string(),
        allocation_id: "alloc_test".to_string(),
        boot_epoch: "epoch_test".to_string(),
        catalog_revision: TEST_REVISION.to_string(),
        profile_revision: TEST_REVISION.to_string(),
        audience_revision: TEST_REVISION.to_string(),
        profile: SelectedLlmProfile {
            selector: "SAAA".to_string(),
            catalog_revision: Some("rev-fixture".to_string()),
            catalog_models: None,
            id: PROFILE_ID.to_string(),
            capability: PROFILE_CAPABILITY.to_string(),
            model: LLM_MODEL.to_string(),
            protocol: "openai.chat-completions.v1".into(),
            context_window: test_context_window(),
            compare_catalog: true,
        },
        created_at: chrono::DateTime::parse_from_rfc3339(created_at).expect("created timestamp"),
        expires_at: chrono::DateTime::parse_from_rfc3339(expires_at).expect("expiry timestamp"),
    }
}
fn claim_json(host: &str, port: u16, audience: &str, expires_at: &str) -> Value {
    let base_url = format!("http://{host}:{port}/v1");
    json!({
        "id": "aconn_test",
        "allocationId": "alloc_test",
        "status": "ready",
        "audience": audience,
        "expiresAt": expires_at,
        "providers": [{
            "name": "llm", "contextWindow": {"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024},
            "capability": PROFILE_CAPABILITY,
            "apiStyle": "openai",
            "protocol": "openai.chat-completions.v1",
            "scheme": "http",
            "host": host,
            "port": port,
            "baseUrl": base_url,
            "model": LLM_MODEL,
            "health": {
                "url": format!("http://{host}:{port}/v1/agent-connections/aconn_test/providers/llm/health"),
                "kind": "semantic-inference",
                "maxAgeMs": 10_000
            },
            "credential": {
                "type": "bearer",
                "token": "short-lived-provider-token",
                "expiresAt": expires_at
            },
            "configuration": {
                "kind": "openai-provider-v1",
                "fields": {
                    "baseURL": base_url,
                    "model": LLM_MODEL
                },
                "secretFields": { "apiKey": "credential.token" }
            }
        }]
    })
}
fn anonymous_claim_json(host: &str, port: u16, audience: &str, expires_at: &str) -> Value {
    let mut claim = claim_json(host, port, audience, expires_at);
    let provider = provider_mut(&mut claim, "llm")
        .as_object_mut()
        .expect("provider object");
    provider.remove("credential");
    provider["configuration"]
        .as_object_mut()
        .expect("configuration object")
        .remove("secretFields");
    claim
}
#[test]
fn derives_control_url_from_host_only() {
    assert_eq!(
        control_base_url("10.0.0.42")
            .expect("private host")
            .as_str(),
        "http://10.0.0.42:9810/"
    );
    assert_eq!(
        control_base_url("dynamic-lan")
            .expect("single-label host")
            .as_str(),
        "http://dynamic-lan:9810/"
    );
}
#[test]
fn rejects_urls_ports_and_public_hosts() {
    for host in [
        "http://dynamic_lan",
        "dynamic_lan:8083",
        "example.com",
        "dynamic_lan/path",
        "-proxy",
        "proxy-",
        "foo..local",
        "foo-.local",
        "[::1]",
    ] {
        assert!(control_base_url(host).is_err(), "{host}");
    }
}
#[test]
fn builds_connection_urls_only_from_bounded_identifier_segments() {
    let base = Url::parse("http://127.0.0.1:9810/").expect("control URL");
    assert_eq!(
        connection_resource_url(&base, "aconn_epoch_uuid")
            .expect("connection URL")
            .as_str(),
        "http://127.0.0.1:9810/v1/agent-connections/aconn_epoch_uuid"
    );
    assert_eq!(
        connection_claim_url(&base, "aconn_epoch_uuid")
            .expect("claim URL")
            .as_str(),
        "http://127.0.0.1:9810/v1/agent-connections/aconn_epoch_uuid/claim"
    );
    for id in [".", "..", "aconn/other", "aconn:other", "aconn.other"] {
        assert!(connection_resource_url(&base, id).is_err(), "{id}");
    }
}
#[test]
fn requires_the_fixed_saaa_desktop_audience() {
    let audiences = vec!["same-host".to_string(), "saaa-desktop".to_string()];
    assert_eq!(select_audience(&audiences).expect("audience"), AUDIENCE);
    assert!(select_audience(&["same-host".to_string()]).is_err());
    assert!(select_audience(&["unknown-network".to_string()]).is_err());
    assert!(select_audience(&[AUDIENCE.to_string(), AUDIENCE.to_string()]).is_err());
}
#[test]
fn accepts_only_the_json_media_type_for_success_responses() {
    assert!(is_json_content_type("application/json"));
    assert!(is_json_content_type("application/json; charset=utf-8"));
    assert!(!is_json_content_type("application/jsonp"));
    assert!(!is_json_content_type("application/problem+json"));
}
#[test]
fn validates_provider_capacity_without_treating_completion_as_guaranteed() {
    let capacity = test_provider_capacity();
    assert!(!capacity.completion_guaranteed);
    assert!(valid_capacity(&capacity));
    assert!(!valid_capacity(&ProviderCapacity {
        max_concurrent_requests: 0,
        ..capacity
    }));
    assert!(!valid_capacity(&ProviderCapacity {
        active_requests: 2,
        ..capacity
    }));
    assert!(!valid_capacity(&ProviderCapacity {
        queue_depth: 3,
        ..capacity
    }));
}
#[tokio::test]
async fn max_concurrent_requests_one_serializes_three_requests() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let capacity = ProviderCapacity {
        max_concurrent_requests: 1,
        ..test_provider_capacity()
    };
    let gate = capacity_gate(&capacity);
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for _ in 0..3 {
        let gate = gate.clone();
        let active = active.clone();
        let peak = peak.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = gate.acquire_owned().await.unwrap();
            let now = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            tokio::task::yield_now().await;
            active.fetch_sub(1, Ordering::SeqCst);
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(peak.load(Ordering::SeqCst), 1);
}
#[test]
fn unauthorized_is_authentication_and_never_provider_unavailable() {
    let error = classify_status(StatusCode::UNAUTHORIZED, "");
    assert_eq!(error.kind, ErrorKind::Authentication);
    assert_ne!(error.kind, ErrorKind::Unavailable);
}
#[test]
fn credential_error_code_contains_no_secret() {
    let error = DynamicLanError::with_code(
        ErrorKind::Authentication,
        "LARM control credential is not safely configured.",
        "credential_missing",
    );
    assert_eq!(error.code(), Some("credential_missing"));
    assert!(!format!("{error:?}").contains("dummy-secret-token"));
}
#[test]
fn accepts_a_private_http_endpoint_and_rejects_loopback_for_remote_dynamic_lan() {
    let (created_at, expires_at) = test_timestamps();
    let identity = test_identity("aconn_test", &created_at, &expires_at);
    let claim = |host: &str| {
        serde_json::from_value::<ConnectionClaim>(claim_json(
            host,
            CONTROL_PORT,
            AUDIENCE,
            &expires_at,
        ))
        .expect("claim fixture")
    };

    let descriptor = validate_claim(claim("10.0.0.42"), &identity, AUDIENCE, false)
        .expect("private HTTP descriptor is accepted");
    assert_eq!(descriptor.base_url, "http://10.0.0.42:9810/v1");
    assert!(validate_claim(claim("127.0.0.1"), &identity, AUDIENCE, false).is_err());
    let wrong_identity = test_identity("different-id", &created_at, &expires_at);
    assert!(validate_claim(claim("10.0.0.42"), &wrong_identity, AUDIENCE, false).is_err());
}
#[test]
fn http_claim_ignores_unused_extensions_but_requires_http_protocol() {
    let (created_at, expires_at) = test_timestamps();
    let identity = test_identity("aconn_test", &created_at, &expires_at);
    let mut value = claim_json("10.0.0.42", CONTROL_PORT, AUDIENCE, &expires_at);
    provider_mut(&mut value, "llm")["streaming"] = json!({"url":"ws://other.local/ignored"});
    assert!(validate_claim(
        serde_json::from_value(value.clone()).unwrap(),
        &identity,
        AUDIENCE,
        false
    )
    .is_ok());
    provider_mut(&mut value, "llm")["protocol"] = json!("saaa.llm-stream.v1");
    assert!(validate_claim(
        serde_json::from_value(value).unwrap(),
        &identity,
        AUDIENCE,
        false
    )
    .is_err());
}
#[test]
fn claim_accepts_explicit_no_auth_without_a_secret_pointer() {
    let (created_at, expires_at) = test_timestamps();
    let identity = test_identity("aconn_test", &created_at, &expires_at);
    let mut value = claim_json("10.0.0.42", CONTROL_PORT, AUDIENCE, &expires_at);
    provider_mut(&mut value, "llm")["credential"] = json!({ "type": "none" });
    provider_mut(&mut value, "llm")["configuration"]
        .as_object_mut()
        .expect("configuration object")
        .remove("secretFields");
    let claim = serde_json::from_value::<ConnectionClaim>(value.clone()).unwrap();
    let descriptor = validate_claim(claim, &identity, AUDIENCE, false)
        .expect("explicit anonymous claim is accepted");
    assert_eq!(descriptor.credential.unwrap().r#type, "none");

    provider_mut(&mut value, "llm")["configuration"]["secretFields"] =
        json!({ "apiKey": "credential.token" });
    let inconsistent = serde_json::from_value::<ConnectionClaim>(value).unwrap();
    assert!(validate_claim(inconsistent, &identity, AUDIENCE, false).is_err());
}
#[test]
fn renewed_expiry_allows_only_the_bounded_clock_skew() {
    let created_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
    let initial_expires_at = (chrono::Utc::now() + chrono::Duration::seconds(45)).to_rfc3339();
    let expected = test_identity("aconn_test", &created_at, &initial_expires_at);
    let within_skew = (chrono::Utc::now() + chrono::Duration::seconds(930)).to_rfc3339();
    let state: ConnectionState = serde_json::from_value(connection_state_json(
        "aconn_test",
        "ready",
        AUDIENCE,
        &created_at,
        &within_skew,
    ))
    .expect("renewed state fixture");
    assert!(validate_renewed_state(&state, &expected, AUDIENCE).is_ok());

    let beyond_skew = (chrono::Utc::now() + chrono::Duration::seconds(961)).to_rfc3339();
    let state: ConnectionState = serde_json::from_value(connection_state_json(
        "aconn_test",
        "ready",
        AUDIENCE,
        &created_at,
        &beyond_skew,
    ))
    .expect("overlong renewed state fixture");
    assert!(validate_renewed_state(&state, &expected, AUDIENCE).is_err());
}
