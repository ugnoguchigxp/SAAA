//! Parsing of the worker's actions and of the raw tool envelopes. Everything is defensive: a
//! shape that does not match is "tool failed", never a success.
use serde::Deserialize;
use serde_json::{json, Value};

use super::checker::extract_json_object;
use super::claims::RawClaim;
use crate::worker_agents::contracts::Coverage;

pub(crate) const MAX_HITS: usize = 10;
pub(crate) const MAX_TITLE_CHARS: usize = 200;
pub(crate) const MAX_SNIPPET_CHARS: usize = 300;
pub(crate) const MAX_PAGE_CHARS: usize = 3000;
const MAX_DATE_CHARS: usize = 40;

/// The one JSON action the worker returns per turn (strict: unknown fields are errors).
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Action {
    WebSearch {
        query: String,
    },
    FetchContent {
        url: String,
        #[serde(default)]
        query: Option<String>,
    },
    Finish {
        claims: Vec<RawClaim>,
        coverage: Coverage,
    },
    GiveUp,
}

pub(crate) fn parse_action(reply: &str) -> Result<Action, String> {
    let json = extract_json_object(reply).ok_or("worker reply has no JSON object")?;
    serde_json::from_str::<Action>(json).map_err(|error| format!("worker action invalid: {error}"))
}

pub(crate) fn clip(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[derive(Debug, Clone)]
pub(crate) struct RawHit {
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub date: Option<String>,
}

#[derive(Debug)]
pub(crate) struct SearchEnvelope {
    pub hits: Vec<RawHit>,
    pub blocked_results: u32,
    /// The search-level guard denied or held the whole result set.
    pub withheld: bool,
}

fn decision_of(value: &Value) -> Option<String> {
    value
        .pointer("/security/decision")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn parse_search(raw: &str) -> Option<SearchEnvelope> {
    let value: Value = serde_json::from_str(raw).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("web_search_result") {
        return None;
    }
    let decision = decision_of(&value)?;
    let hits = value
        .get("hits")?
        .as_array()?
        .iter()
        .filter_map(|hit| {
            Some(RawHit {
                url: hit.get("url")?.as_str()?.to_string(),
                title: clip(
                    hit.get("title").and_then(Value::as_str).unwrap_or(""),
                    MAX_TITLE_CHARS,
                ),
                snippet: clip(
                    hit.get("snippet").and_then(Value::as_str).unwrap_or(""),
                    MAX_SNIPPET_CHARS,
                ),
                date: ["publishedAt", "date"]
                    .iter()
                    .find_map(|key| hit.get(*key).and_then(Value::as_str))
                    .map(|date| clip(date, MAX_DATE_CHARS))
                    .filter(|date| !date.is_empty()),
            })
        })
        .take(MAX_HITS)
        .collect();
    Some(SearchEnvelope {
        hits,
        blocked_results: value
            .get("blockedResultCount")
            .and_then(Value::as_u64)
            .map_or(0, |count| count.min(1000) as u32),
        withheld: !matches!(decision.as_str(), "allow" | "allow_with_warning"),
    })
}

#[derive(Debug)]
pub(crate) struct FetchEnvelope {
    pub decision: String,
    pub warning_categories: Vec<String>,
    pub retrieval_status: String,
    pub final_url: Option<String>,
    pub fetched_at: Option<String>,
    pub truncated: bool,
    pub text: String,
}

pub(crate) fn parse_fetch(raw: &str) -> Option<FetchEnvelope> {
    let value: Value = serde_json::from_str(raw).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("fetch_content_result") {
        return None;
    }
    let decision = decision_of(&value)?;
    let document = value.get("document")?;
    let warning_categories = value
        .pointer("/security/warningCategories")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(FetchEnvelope {
        decision,
        warning_categories,
        retrieval_status: document
            .get("retrievalStatus")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        final_url: document
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string),
        fetched_at: document
            .get("fetchedAt")
            .and_then(Value::as_str)
            .map(|date| clip(date, MAX_DATE_CHARS)),
        truncated: document
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        text: clip(
            document.get("text").and_then(Value::as_str).unwrap_or(""),
            MAX_PAGE_CHARS,
        ),
    })
}

impl FetchEnvelope {
    /// Plugin flag (§6.3): deny / require_approval, a blocked retrieval, or a warning with any
    /// category other than `low_trust_attribute`. An unknown decision fails closed.
    pub(crate) fn plugin_flagged(&self) -> bool {
        match self.decision.as_str() {
            "allow" => self.retrieval_status == "blocked",
            "allow_with_warning" => {
                self.retrieval_status == "blocked"
                    || self
                        .warning_categories
                        .iter()
                        .any(|category| category != "low_trust_attribute")
            }
            _ => true,
        }
    }
}

/// Data envelope shown to the worker for search results.
pub(crate) fn hits_envelope(hits: &[(String, RawHit)]) -> Value {
    json!({
        "type": "search_results",
        "trust": "untrusted_data",
        "hits": hits.iter().map(|(url, hit)| json!({
            "url": url,
            "title": hit.title,
            "snippet": hit.snippet,
            "date": hit.date,
        })).collect::<Vec<_>>(),
    })
}

pub(crate) fn page_envelope(url: &str, page: &FetchEnvelope) -> Value {
    json!({
        "type": "page",
        "trust": "untrusted_data",
        "url": url,
        "fetchedAt": page.fetched_at,
        "truncated": page.truncated,
        "text": page.text,
    })
}
