use std::env;
mod credentials;
mod urls;

pub(crate) fn bounded_text(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn redact_runtime_text(value: &str) -> String {
    let secrets = env::vars()
        .filter_map(|(key, value)| {
            let key = key.to_ascii_uppercase();
            ((key.ends_with("_API_KEY") || key.ends_with("_TOKEN")) && !value.is_empty())
                .then_some(value)
        })
        .collect();
    redact_with_secrets(value, secrets)
}

fn redact_with_secrets(value: &str, mut secrets: Vec<String>) -> String {
    secrets.sort_unstable_by(|left, right| right.len().cmp(&left.len()).then(left.cmp(right)));
    secrets.dedup();
    let mut redacted = value.to_string();
    for secret in secrets {
        redacted = redacted.replace(&secret, "[REDACTED]");
    }
    bounded_text(&credentials::redact(&redacted), 2_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_errors_are_redacted_and_bounded() {
        let redacted = redact_with_secrets(
            &format!("token=super-secret-test-value {}", "x".repeat(4_000)),
            vec!["super-secret-test-value".to_string()],
        );
        assert!(!redacted.contains("super-secret-test-value"));
        assert!(redacted.contains("[REDACTED]"));
        assert!(redacted.chars().count() <= 2_000);
    }

    #[test]
    fn response_credentials_are_removed_without_erasing_recovery_information() {
        let value = "Authentication failed: Bearer ephemeral-example; api_key=key-example https://user:pass@example.org/retry?token=query-example retry in 2s";
        let result = redact_with_secrets(value, vec![]);
        for secret in [
            "ephemeral-example",
            "key-example",
            "query-example",
            "user:pass",
        ] {
            assert!(!result.contains(secret));
        }
        assert!(result.contains("Authentication failed"));
        assert!(result.contains("retry in 2s"));
        assert!(result.contains("example.org/retry"));
    }

    #[test]
    fn json_credentials_never_reach_ui_errors() {
        let input = r#"Provider error: {"access_token":"review secret", "nested":{"api_key":"quote\"and\\suffix"}, "password":"a b", "reason":"authentication failed", "retryAfter":2}"#;
        let result = redact_with_secrets(input, vec![]);
        for secret in ["review secret", "quote", "suffix", "a b"] {
            assert!(!result.contains(secret), "credential survived: {result}");
        }
        assert!(result.contains("authentication failed"));
        assert!(result.contains("retryAfter"));
        let parsed: serde_json::Value =
            serde_json::from_str(result.strip_prefix("Provider error: ").unwrap()).unwrap();
        assert_eq!(parsed["access_token"], "[REDACTED]");
        assert_eq!(parsed["nested"]["api_key"], "[REDACTED]");
    }

    #[test]
    fn longer_overlapping_secrets_are_redacted_before_prefixes() {
        let redacted = redact_with_secrets(
            "first=token-prefix-suffix second=token-prefix",
            vec![
                "token-prefix".to_string(),
                "token-prefix-suffix".to_string(),
                "token-prefix".to_string(),
            ],
        );
        assert_eq!(redacted, "first=[REDACTED] second=[REDACTED]");
        assert!(!redacted.contains("suffix"));
    }
}
