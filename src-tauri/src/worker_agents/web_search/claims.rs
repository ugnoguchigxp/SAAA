//! Host-side validation of the worker's `finish` claims (§6.4). The claims are the only text of
//! web origin that reaches the conversation agent, so they must be short declarative sentences
//! bound to host-recorded usable sources. Anything else is dropped, never repaired.
use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use tauri_plugin_llm_fetch::{inspect_plain_text_bounded, GuardDecision};

use super::sources::{SourceBook, SourceKind, SourceStatus};
use crate::worker_agents::contracts::{
    AttemptError, ClaimBasis, Confidence, Coverage, ExcludedSummary, WebClaim, WebClaims,
};

pub(crate) const MAX_CLAIMS: usize = 8;
pub(crate) const MAX_CLAIM_CHARS: usize = 240;
const GUARD_CHARS: usize = 4000;

/// A claim as the worker wrote it in the `finish` action.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RawClaim {
    pub text: String,
    pub source_url: String,
    pub basis: ClaimBasis,
    /// Accepted for compatibility with the worker protocol but never trusted: dates come from
    /// the host's own records.
    #[serde(default)]
    #[allow(dead_code)]
    pub published_or_fetched_at: Option<String>,
}

/// Role and addressing words (case-insensitive). Claims are statements about the world, never
/// messages to an assistant, so words that address or redefine one are rejected.
const ROLE_WORD_PATTERNS: &[&str] = &[
    r"\b(system|assistant|user|developer)\b",
    r"\bAI (assistant|agent|model)\b",
    r"\bas an AI\b",
    r"\b(ignore|disregard|forget|override)\b[^.]{0,30}\b(previous|prior|above|earlier|all|any|instructions?|rules?)\b",
    r"\b(new|updated) instructions?\b",
    r"\byou (must|should|need to|have to|are|will)\b",
    r"\b(prompt|jailbreak)\b",
    r"あなた|ユーザー|指示|無視|命令|プロンプト|システム(メッセージ|プロンプト)|アシスタント",
];

/// Leading English imperative verbs at the start of a sentence.
const IMPERATIVE_VERBS: &[&str] = &[
    "ignore",
    "disregard",
    "forget",
    "do",
    "don't",
    "never",
    "always",
    "please",
    "call",
    "run",
    "execute",
    "send",
    "reveal",
    "print",
    "output",
    "repeat",
    "say",
    "tell",
    "write",
    "open",
    "click",
    "visit",
    "fetch",
    "search",
    "use",
    "follow",
    "respond",
    "reply",
    "answer",
    "act",
    "pretend",
    "remember",
    "ensure",
    "make",
    "give",
    "show",
    "stop",
    "start",
    "return",
    "delete",
    "remove",
    "update",
    "add",
    "switch",
    "change",
    "summarize",
    "translate",
    "ask",
    "stay",
    "keep",
    "treat",
    "assume",
    "trust",
    "obey",
    "must",
    "should",
];

static ROLE_WORDS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    ROLE_WORD_PATTERNS
        .iter()
        .map(|pattern| Regex::new(&format!("(?i){pattern}")).expect("role pattern is valid"))
        .collect()
});

static JA_FINAL_IMPERATIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(ください|下さい|しろ|せよ|なさい)[\s。．.]*$")
        .expect("imperative pattern is valid")
});

static SENTENCE_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[.!?。]\s*)([A-Za-z']+)").expect("sentence pattern is valid")
});

const FORBIDDEN_SUBSTRINGS: &[&str] = &["http", "www.", "tool_result", "host_", "worker_"];
const FORBIDDEN_CHARS: &[char] = &['[', ']', '<', '>', '`'];

fn is_invisible_format_char(character: char) -> bool {
    matches!(character,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// Shape rules only (no source or guard checks). Returns the first violated rule.
pub(crate) fn text_violation(text: &str) -> Option<&'static str> {
    let count = text.chars().count();
    if count == 0 || count > MAX_CLAIM_CHARS {
        return Some("length");
    }
    if text
        .chars()
        .any(|c| c.is_control() || is_invisible_format_char(c))
    {
        return Some("control_character");
    }
    if text != text.trim() {
        return Some("whitespace");
    }
    if text.chars().any(|c| FORBIDDEN_CHARS.contains(&c)) {
        return Some("markup");
    }
    let lower = text.to_lowercase();
    if FORBIDDEN_SUBSTRINGS
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return Some("url_or_marker");
    }
    if ROLE_WORDS.iter().any(|pattern| pattern.is_match(text)) {
        return Some("role_word");
    }
    if text.contains('!') || text.contains('！') || JA_FINAL_IMPERATIVE.is_match(text) {
        return Some("imperative");
    }
    if SENTENCE_START.captures_iter(text).any(|captures| {
        let word = captures[1].to_lowercase();
        IMPERATIVE_VERBS.contains(&word.as_str())
    }) {
        return Some("imperative");
    }
    None
}

fn guard_allows(text: &str) -> bool {
    inspect_plain_text_bounded(text, GUARD_CHARS).decision == GuardDecision::Allow
}

/// `Ok` carries claims bound to usable sources; dropped claims are silently excluded.
pub(crate) fn validate_claims(
    raw: Vec<RawClaim>,
    book: &SourceBook,
    min_items: u32,
    coverage: Coverage,
) -> Result<WebClaims, AttemptError> {
    if raw.is_empty() || raw.len() > MAX_CLAIMS {
        return Err(AttemptError::InvalidOutput(format!(
            "claims must be 1..={MAX_CLAIMS}, got {}",
            raw.len()
        )));
    }
    let mut kept: Vec<WebClaim> = Vec::new();
    let mut hosts: Vec<String> = Vec::new();
    for claim in raw {
        if text_violation(&claim.text).is_some() || !guard_allows(&claim.text) {
            continue;
        }
        let kind = match claim.basis {
            ClaimBasis::Page => SourceKind::Fetched,
            ClaimBasis::Snippet => SourceKind::SearchHit,
        };
        let Some(entry) = book
            .entry(kind, &claim.source_url)
            .filter(|entry| entry.status == SourceStatus::Usable)
        else {
            continue;
        };
        hosts.push(entry.host.clone());
        kept.push(WebClaim {
            text: claim.text,
            source_url: claim.source_url,
            basis: claim.basis,
            published_or_fetched_at: entry.date.clone(),
        });
    }
    let joined = kept
        .iter()
        .map(|claim| claim.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if kept.is_empty() || !guard_allows(&joined) {
        return Err(AttemptError::CompletionUnmet);
    }
    if (kept.len() as u32) < min_items {
        return Err(AttemptError::CompletionUnmet);
    }
    let confidence = confidence_of(&kept, &hosts);
    Ok(WebClaims {
        claims: kept,
        excluded: excluded_summary(book),
        confidence,
        coverage,
    })
}

/// Host-computed: >= 2 distinct hosts => corroborated; otherwise a page-based claim =>
/// single_source; otherwise snippet_only.
pub(crate) fn confidence_of(claims: &[WebClaim], hosts: &[String]) -> Confidence {
    let distinct: HashSet<&String> = hosts.iter().collect();
    if distinct.len() >= 2 {
        Confidence::Corroborated
    } else if claims.iter().any(|claim| claim.basis == ClaimBasis::Page) {
        Confidence::SingleSource
    } else {
        Confidence::SnippetOnly
    }
}

pub(crate) fn excluded_summary(book: &SourceBook) -> ExcludedSummary {
    ExcludedSummary {
        count: book.excluded_count(),
        categories: book.categories(),
        domains: book.excluded_domains(),
    }
}
