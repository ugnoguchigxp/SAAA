use std::env;

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
    bounded_text(&redacted, 2_000)
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
