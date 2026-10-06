//! Redaction of checker excerpts before they are stored (§6.6). Page bodies are never stored;
//! the short suspicious excerpt is, after secrets and personal identifiers are masked.
use regex::Regex;
use std::sync::LazyLock;

pub(crate) const MAX_EXCERPT_CHARS: usize = 240;
const REDACTED: &str = "[redacted]";

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // Bearer / API-key like tokens first so their digits are not split by later rules.
        r"(?i)bearer\s+[A-Za-z0-9._~+/=\-]{6,}",
        r"\bsk-[A-Za-z0-9_\-]{6,}",
        r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}",
        // Digit groups such as card or phone numbers: 9+ digits, optionally separated.
        r"\d(?:[ \-]?\d){8,}",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("redaction pattern is valid"))
    .collect()
});

/// Masks emails, `sk-` / `Bearer` tokens and long digit runs (>= 9 digits), flattens control
/// characters and caps the result at 240 characters.
pub(crate) fn redact_excerpt(text: &str) -> String {
    let mut value: String = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    for pattern in PATTERNS.iter() {
        value = pattern.replace_all(&value, REDACTED).into_owned();
    }
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    value.chars().take(MAX_EXCERPT_CHARS).collect()
}
