//! Injection checker (§6.5): a separate, tool-less model call that judges untrusted text.
//! Fail-closed everywhere: truncation, timeout, a model error or unparsable output all count as
//! "suspected". The checker's output never enters the worker transcript.
use std::time::Duration;

use serde::Deserialize;

use crate::worker_agents::contracts::{TierRoute, WorkerModel};
use crate::RunCancellation;

pub(crate) const CHECKER_CONTEXT: &str =
    include_str!("../../../../.s11tnext/worker-injection-check.txt");
pub(crate) const MAX_CHECK_CHARS: usize = 3000;
pub(crate) const MAX_EXCERPT_CHARS: usize = 160;
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_CATEGORIES: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckVerdict {
    pub suspected: bool,
    pub categories: Vec<String>,
    /// Unredacted, <= 160 chars. Redaction happens when the audit row is written.
    pub excerpt: String,
    pub failed_closed: bool,
}

impl CheckVerdict {
    fn fail_closed() -> Self {
        Self {
            suspected: true,
            categories: Vec::new(),
            excerpt: String::new(),
            failed_closed: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchVerdict {
    pub flagged: Vec<usize>,
    pub categories: Vec<String>,
    pub failed_closed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SingleOutput {
    suspected: bool,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    excerpt: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchOutput {
    flagged: Vec<usize>,
    #[serde(default)]
    categories: Vec<String>,
}

pub(crate) struct Checker<'a> {
    pub model: &'a dyn WorkerModel,
    pub route: &'a TierRoute,
    pub cancellation: &'a RunCancellation,
    pub timeout: Duration,
}

impl<'a> Checker<'a> {
    pub(crate) fn new(
        model: &'a dyn WorkerModel,
        route: &'a TierRoute,
        cancellation: &'a RunCancellation,
    ) -> Self {
        Self {
            model,
            route,
            cancellation,
            timeout: CHECK_TIMEOUT,
        }
    }

    /// One untrusted text (a fetched page). Over `MAX_CHECK_CHARS` the text is cut, marked as
    /// truncated and the verdict is forced to suspected.
    pub(crate) async fn check_text(&self, text: &str) -> CheckVerdict {
        let (body, truncated) = truncate(text, MAX_CHECK_CHARS);
        let input = wrap("single", &body);
        let verdict = match self.ask(&input).await {
            Some(raw) => parse_single(&raw),
            None => CheckVerdict::fail_closed(),
        };
        if truncated {
            return CheckVerdict {
                suspected: true,
                failed_closed: true,
                ..verdict
            };
        }
        verdict
    }

    /// Numbered search-hit texts (title + snippet). Indexes in the answer are 0-based.
    pub(crate) async fn check_batch(&self, items: &[String]) -> BatchVerdict {
        let mut body = String::new();
        for (index, item) in items.iter().enumerate() {
            let (text, _) = truncate(item, MAX_CHECK_CHARS / 4);
            body.push_str(&format!("[{index}] {}\n", text.replace('\n', " ")));
        }
        let input = wrap("batch", &body);
        match self.ask(&input).await {
            Some(raw) => parse_batch(&raw, items.len()),
            None => BatchVerdict {
                flagged: (0..items.len()).collect(),
                categories: Vec::new(),
                failed_closed: true,
            },
        }
    }

    async fn ask(&self, input: &str) -> Option<String> {
        self.model
            .complete(
                self.route,
                CHECKER_CONTEXT,
                input,
                self.cancellation,
                self.timeout,
            )
            .await
            .ok()
    }
}

fn truncate(text: &str, max_chars: usize) -> (String, bool) {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        (format!("{head}\n[truncated]"), true)
    } else {
        (head, false)
    }
}

/// The untrusted text sits between two boundary lines carrying the same fresh nonce.
fn wrap(mode: &str, body: &str) -> String {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    // The nonce is unpredictable to the page author; strip it anyway if it ever appears.
    let body = body.replace(&nonce, "");
    format!("MODE: {mode}\n===== UNTRUSTED {nonce} =====\n{body}\n===== UNTRUSTED {nonce} =====")
}

/// Extracts the JSON object from a reply that may carry code fences or stray prose.
pub(crate) fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    (end > start).then(|| &raw[start..=end])
}

/// Keeps only names of the plugin's `SecurityFindingCategory`; unknown names are dropped.
fn known_categories(names: Vec<String>) -> Vec<String> {
    let mut known: Vec<String> = names
        .into_iter()
        .filter_map(|name| {
            serde_json::from_value::<tauri_plugin_llm_fetch::SecurityFindingCategory>(
                serde_json::Value::String(name),
            )
            .ok()
        })
        .filter_map(|category| match serde_json::to_value(category) {
            Ok(serde_json::Value::String(name)) => Some(name),
            _ => None,
        })
        .collect();
    known.sort();
    known.dedup();
    known.truncate(MAX_CATEGORIES);
    known
}

pub(crate) fn parse_single(raw: &str) -> CheckVerdict {
    let Some(json) = extract_json_object(raw) else {
        return CheckVerdict::fail_closed();
    };
    match serde_json::from_str::<SingleOutput>(json) {
        Ok(output) => CheckVerdict {
            suspected: output.suspected,
            categories: known_categories(output.categories),
            excerpt: output
                .excerpt
                .chars()
                .take(MAX_EXCERPT_CHARS)
                .collect::<String>(),
            failed_closed: false,
        },
        Err(_) => CheckVerdict::fail_closed(),
    }
}

pub(crate) fn parse_batch(raw: &str, item_count: usize) -> BatchVerdict {
    let closed = || BatchVerdict {
        flagged: (0..item_count).collect(),
        categories: Vec::new(),
        failed_closed: true,
    };
    let Some(json) = extract_json_object(raw) else {
        return closed();
    };
    let Ok(output) = serde_json::from_str::<BatchOutput>(json) else {
        return closed();
    };
    if output.flagged.iter().any(|index| *index >= item_count) {
        return closed();
    }
    let mut flagged = output.flagged;
    flagged.sort_unstable();
    flagged.dedup();
    BatchVerdict {
        flagged,
        categories: known_categories(output.categories),
        failed_closed: false,
    }
}
