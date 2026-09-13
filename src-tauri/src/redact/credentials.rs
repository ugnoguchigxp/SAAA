use regex::Regex;
use std::sync::OnceLock;

pub(super) fn redact(value: &str) -> String {
    let mut redacted = value.to_string();
    // Authentication material can come from a provider response or Keychain rather
    // than the process environment. Strip explicit credential syntax before bounding.
    // Consume the whole JSON string, including whitespace and escaped quotes.
    static JSON_CREDENTIAL: OnceLock<Regex> = OnceLock::new();
    let json_credential = JSON_CREDENTIAL.get_or_init(|| Regex::new(
        r#"(?i)("(?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|password|credential)"\s*:\s*)"(?:\\.|[^"\\])*""#,
    ).expect("static JSON credential pattern"));
    redacted = json_credential
        .replace_all(&redacted, "${1}\"[REDACTED]\"")
        .into_owned();
    static CREDENTIAL: OnceLock<Regex> = OnceLock::new();
    let credential = CREDENTIAL.get_or_init(|| Regex::new(
        r#"(?i)(\bbearer\s+|\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|secret|password|credential)\s*[=:]\s*[\"']?)[^\s\"'&,;<>]+"#,
    ).expect("static credential pattern"));
    redacted = credential
        .replace_all(&redacted, "${1}[REDACTED]")
        .into_owned();
    super::urls::redact(&redacted)
}
