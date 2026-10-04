use super::*;

pub(crate) fn control_token() -> Result<String, DynamicLanError> {
    #[cfg(not(test))]
    let loaded = crate::providers::dynamic_lan::credential::load();
    #[cfg(test)]
    let loaded = crate::providers::dynamic_lan::credential::load_environment_only_for_test();
    loaded
        .map(|loaded| loaded.token().to_string())
        .map_err(|error| {
            DynamicLanError::with_code(
                ErrorKind::Authentication,
                "LARM control credential is not safely configured.",
                error.code(),
            )
        })
}

pub(crate) fn control_credential() -> Result<HeaderValue, DynamicLanError> {
    #[cfg(not(test))]
    let loaded = crate::providers::dynamic_lan::credential::load();
    #[cfg(test)]
    let loaded = crate::providers::dynamic_lan::credential::load_environment_only_for_test();
    let loaded = loaded.map_err(|error| {
        DynamicLanError::with_code(
            ErrorKind::Authentication,
            "LARM control credential is not safely configured.",
            error.code(),
        )
    })?;
    provider_credential(loaded.token()).map_err(|_| {
        DynamicLanError::with_code(
            ErrorKind::Authentication,
            "LARM control credential is invalid.",
            "credential_invalid",
        )
    })
}

pub(crate) fn provider_credential(token: &str) -> Result<HeaderValue, DynamicLanError> {
    if token.is_empty()
        || token.len() > 4_096
        || token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(DynamicLanError::new(
            ErrorKind::Authentication,
            "The dynamic LAN provider credential is invalid.",
        ));
    }
    let bearer = Zeroizing::new(format!("Bearer {token}"));
    let mut value = HeaderValue::from_str(bearer.as_str()).map_err(|_| {
        DynamicLanError::new(
            ErrorKind::Authentication,
            "The dynamic LAN provider credential is invalid.",
        )
    })?;
    value.set_sensitive(true);
    Ok(value)
}

pub(crate) fn classify_transport(error: reqwest::Error) -> DynamicLanError {
    if error.is_timeout() {
        DynamicLanError::new(
            ErrorKind::Timeout,
            "The dynamic_lan configuration API request timed out.",
        )
    } else {
        DynamicLanError::new(
            ErrorKind::Network,
            "Could not reach the dynamic_lan configuration API.",
        )
    }
}

pub(crate) fn classify_status(status: StatusCode, code: &str) -> DynamicLanError {
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return DynamicLanError::new(
            ErrorKind::Authentication,
            "dynamic_lan rejected the connection authorization.",
        );
    }
    if matches!(status, StatusCode::NOT_FOUND | StatusCode::GONE) {
        return DynamicLanError::new(
            ErrorKind::StaleConnection,
            "The dynamic LAN provider connection is no longer active.",
        );
    }
    if status == StatusCode::CONFLICT {
        return classify_api_error(code);
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return DynamicLanError::new(
            ErrorKind::Capacity,
            "dynamic_lan cannot allocate the requested provider right now.",
        );
    }
    if status == StatusCode::SERVICE_UNAVAILABLE {
        return DynamicLanError::with_code(
            ErrorKind::Unavailable,
            "The dynamic LAN provider service is not ready.",
            "larm_provider_terminal",
        );
    }
    if status.is_server_error() {
        return DynamicLanError::new(
            ErrorKind::Upstream,
            "dynamic_lan could not resolve the provider connection.",
        );
    }
    classify_api_error(code)
}

pub(crate) fn classify_api_error(code: &str) -> DynamicLanError {
    match code {
        "catalog_revision_mismatch" | "revision_mismatch" => DynamicLanError::with_code(
            ErrorKind::Contract,
            "LARM catalog revision changed before provide.",
            "larm_revision_mismatch",
        ),
        "idempotency_conflict" => DynamicLanError::with_code(
            ErrorKind::Contract,
            "LARM rejected an idempotency key conflict.",
            "larm_idempotency_conflict",
        ),
        "unknown_profile" | "unknown_selector" => DynamicLanError::with_code(
            ErrorKind::Contract,
            "LARM rejected the requested selector.",
            "larm_unknown_selector",
        ),
        "provider_conflict" | "connection_audience_unavailable" => DynamicLanError::with_code(
            ErrorKind::Capacity,
            "LARM provider is reserved by another consumer.",
            "larm_provider_conflict",
        ),
        "capacity_exhausted" | "admission_denied" | "provider_busy" => DynamicLanError::new(
            ErrorKind::Capacity,
            "dynamic_lan cannot allocate the requested provider right now.",
        ),
        "connection_auth_not_configured" | "unauthorized" | "forbidden" => DynamicLanError::new(
            ErrorKind::Authentication,
            "dynamic_lan rejected the connection authorization.",
        ),
        "provider_semantic_not_ready" | "connection_not_ready" => DynamicLanError::new(
            ErrorKind::Unavailable,
            "The dynamic LAN provider did not pass semantic readiness checks.",
        ),
        "connection_inactive"
        | "connection_idle_released"
        | "foreground_idle_timeout"
        | "connection_expired"
        | "connection_released"
        | "connection_boot_epoch_mismatch"
        | "connection_not_found" => DynamicLanError::new(
            ErrorKind::StaleConnection,
            "The dynamic LAN provider connection is no longer active.",
        ),
        _ => contract_error(()),
    }
}

#[cfg(test)]
mod response_diagnostic_tests {
    use super::*;
    use std::io::{Read, Write};

    #[tokio::test]
    async fn malformed_catalog_preserves_stage_without_exposing_response_text() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let read = socket.read(&mut request).unwrap();
            assert!(read > 0);
            let body = r#"{"private":"DO_NOT_PERSIST_PROVIDER_TEXT"}"#;
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let result = send_json_response::<crate::providers::dynamic_lan::AgentProfileCatalog>(
            &reqwest::Client::new(),
            Method::GET,
            Url::parse(&format!("http://{address}/v3/agent-profiles")).unwrap(),
            None,
            None,
            None,
            &RunCancellation::default(),
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("malformed catalog accepted"),
        };
        assert_eq!(error.public_message(), "harness-catalog-schema-invalid");
        assert!(!format!("{error:?}").contains("DO_NOT_PERSIST"));
        server.join().unwrap();
    }
}
