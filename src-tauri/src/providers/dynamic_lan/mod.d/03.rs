use super::*;
use serde_json::Value;
use std::env;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
const PROFILE_CAPABILITY: &str = "llm.reasoning";
const TEST_REVISION: &str = "0000000000000000000000000000000000000000000000000000000000000000";
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
            "profile": PROFILE_SELECTOR,
            "agentProfile": AGENT_PROFILE,
            "profileRevision": TEST_REVISION,
            "audience": audience,
            "audienceRevision": TEST_REVISION,
            "status": status,
            "providers": [{
                "name": "llm", "contextWindow": {"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024},
                "capability": PROFILE_CAPABILITY,
                "supportedCapabilities": [PROFILE_CAPABILITY],
                "protocol": "openai.chat-completions.v1",
                "endpoint": "/v1/chat/completions",
                "model": AGENT_PROFILE,
                "readiness": readiness,
                "claimable": status == "ready"
            }],
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
                id: AGENT_PROFILE.to_string(),
                capability: PROFILE_CAPABILITY.to_string(),
                model: AGENT_PROFILE.to_string(),
                context_window: test_context_window(),
            },
            created_at: chrono::DateTime::parse_from_rfc3339(created_at)
                .expect("created timestamp"),
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
                "model": AGENT_PROFILE,
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
                        "model": AGENT_PROFILE
                    },
                    "secretFields": { "apiKey": "credential.token" }
                }
            }]
        })
    }
fn anonymous_claim_json(host: &str, port: u16, audience: &str, expires_at: &str) -> Value {
        let mut claim = claim_json(host, port, audience, expires_at);
        let provider = claim["providers"][0]
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
    fn accepts_the_legacy_profile_when_no_default_is_advertised() {
        let profile = || CatalogAgentProfile {
            id: AGENT_PROFILE.to_string(),
            legacy_profile_context_window: Some(test_context_window()),
            providers: vec![CatalogProvider {
                context_window: None,
                name: "llm".to_string(),
                capability: PROFILE_CAPABILITY.to_string(),
                supported_capabilities: Vec::new(),
                protocol: "openai.chat-completions.v1".to_string(),
                model: AGENT_PROFILE.to_string(),
            }],
        };
        let profiles = AgentProfileCatalog {
            contract_version: "agent-connection.v1".to_string(),
            default_agent_profile: None,
            requested_profile: None,
            profiles: vec![profile()],
            audiences: vec!["saaa-desktop".to_string()],
        };
        assert_eq!(
            select_default_llm_profile(&profiles).expect("legacy profile remains compatible"),
            SelectedLlmProfile {
                id: AGENT_PROFILE.to_string(),
                capability: PROFILE_CAPABILITY.to_string(),
                model: AGENT_PROFILE.to_string(),
                context_window: test_context_window(),
            }
        );

        let duplicate = AgentProfileCatalog {
            contract_version: profiles.contract_version.clone(),
            default_agent_profile: None,
            requested_profile: None,
            profiles: vec![profile(), profile()],
            audiences: profiles.audiences.clone(),
        };
        assert!(select_default_llm_profile(&duplicate).is_err());
    }
#[test]
    fn selects_the_advertised_default_compatible_profile() {
        let profiles = serde_json::from_value::<AgentProfileCatalog>(json!({
            "contractVersion": "agent-connection.v1",
            "defaultAgentProfile": "coding-default",
            "profiles": [{
                "id": "coding-default",
                "providers": [{
                    "name": "llm", "contextWindow": {"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024},
                    "capability": "llm.coding",
                    "supportedCapabilities": ["llm.coding", "llm.general", "llm.reasoning"],
                    "protocol": "openai.chat-completions.v1",
                    "model": "coding-default"
                }]
            }],
            "audiences": [AUDIENCE]
        }))
        .expect("current agent profile descriptor decodes");

        assert_eq!(
            select_default_llm_profile(&profiles).expect("compatible default profile is selected"),
            SelectedLlmProfile {
                id: "coding-default".to_string(),
                capability: "llm.coding".to_string(),
                model: "coding-default".to_string(),
                context_window: test_context_window(),
            }
        );
    }
#[test]
    fn selected_profile_can_bind_a_different_public_model() {
        let profiles = serde_json::from_value::<AgentProfileCatalog>(json!({
            "contractVersion": "agent-connection.v1",
            "profiles": [{
                "id": AGENT_PROFILE,
                "providers": [{
                    "name": "llm", "contextWindow": {"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024},
                    "capability": PROFILE_CAPABILITY,
                    "protocol": "openai.chat-completions.v1",
                    "model": "coding-default"
                }]
            }],
            "audiences": [AUDIENCE]
        }))
        .expect("compatibility alias descriptor decodes");

        assert_eq!(
            select_default_llm_profile(&profiles).expect("compatibility alias is selected"),
            SelectedLlmProfile {
                id: AGENT_PROFILE.to_string(),
                capability: PROFILE_CAPABILITY.to_string(),
                model: "coding-default".to_string(),
                context_window: test_context_window(),
            }
        );
    }
#[test]
    fn current_v3_profile_catalog_is_accepted() {
        let profiles: AgentProfileCatalog = serde_json::from_value(json!({
            "contractVersion": "agent-connection.v3", "requestedProfile": "SAAA",
            "defaultAgentProfile": "coding-default",
            "profiles": [{
                "id": "coding-default",
                "providers": [{
                    "name": "llm", "contextWindow": {"maxTokens":230400,"outputReserveTokens":4096,"safetyMarginTokens":1976},
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
        }))
        .expect("v3 catalog deserializes");

        let selected = select_default_llm_profile(&profiles).expect("v3 catalog is compatible");
        assert_eq!(selected.id, "coding-default");
        assert_eq!(selected.model, "coding-default");
        assert_eq!(selected.capability, "llm.coding");
        assert_eq!(selected.context_window.max_tokens, 230400);
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
    fn validates_profile_context_budget() {
        let profiles = |max_tokens, output_reserve_tokens, safety_margin_tokens| {
            serde_json::from_value::<AgentProfileCatalog>(json!({
                "contractVersion": "agent-connection.v1",
                "profiles": [{
                    "id": AGENT_PROFILE,
                    "contextWindow": {
                        "maxTokens": max_tokens,
                        "outputReserveTokens": output_reserve_tokens,
                        "safetyMarginTokens": safety_margin_tokens
                    },
                    "providers": [{
                        "name": "llm", "contextWindow": {"maxTokens":32768,"outputReserveTokens":4096,"safetyMarginTokens":1024},
                        "capability": PROFILE_CAPABILITY,
                        "protocol": "openai.chat-completions.v1",
                        "model": AGENT_PROFILE
                    }]
                }],
                "audiences": [AUDIENCE]
            }))
            .unwrap()
        };
        let selected = select_default_llm_profile(&profiles(32_768, 4_096, 1_024)).unwrap();
        assert_eq!(selected.context_window, test_context_window());
        assert!(select_default_llm_profile(&profiles(0, 4_096, 1_024)).is_err());
        assert!(select_default_llm_profile(&profiles(4_096, 4_096, 1)).is_err());
        assert!(select_default_llm_profile(&profiles(4_096, 3_000, 2_000)).is_err());
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
        value["providers"][0]["streaming"] = json!({"url":"ws://other.local/ignored"});
        assert!(validate_claim(
            serde_json::from_value(value.clone()).unwrap(),
            &identity,
            AUDIENCE,
            false
        )
        .is_ok());
        value["providers"][0]["protocol"] = json!("saaa.llm-stream.v1");
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
        value["providers"][0]["credential"] = json!({ "type": "none" });
        value["providers"][0]["configuration"]
            .as_object_mut()
            .expect("configuration object")
            .remove("secretFields");
        let claim = serde_json::from_value::<ConnectionClaim>(value.clone()).unwrap();
        let descriptor = validate_claim(claim, &identity, AUDIENCE, false)
            .expect("explicit anonymous claim is accepted");
        assert_eq!(descriptor.credential.unwrap().r#type, "none");

        value["providers"][0]["configuration"]["secretFields"] =
            json!({ "apiKey": "credential.token" });
        let inconsistent = serde_json::from_value::<ConnectionClaim>(value).unwrap();
        assert!(validate_claim(inconsistent, &identity, AUDIENCE, false).is_err());
    }
#[test]
    fn renewed_expiry_allows_only_the_bounded_clock_skew() {
        let created_at = (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339();
        let initial_expires_at = (chrono::Utc::now() + chrono::Duration::seconds(45)).to_rfc3339();
        let expected = test_identity("aconn_test", &created_at, &initial_expires_at);
        let within_skew = (chrono::Utc::now() + chrono::Duration::seconds(330)).to_rfc3339();
        let state: ConnectionState = serde_json::from_value(connection_state_json(
            "aconn_test",
            "ready",
            AUDIENCE,
            &created_at,
            &within_skew,
        ))
        .expect("renewed state fixture");
        assert!(validate_renewed_state(&state, &expected, AUDIENCE).is_ok());

        let beyond_skew = (chrono::Utc::now() + chrono::Duration::seconds(361)).to_rfc3339();
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
