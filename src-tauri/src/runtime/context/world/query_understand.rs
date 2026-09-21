//! Natural-language World graph query understanding. Host-validated, Scope-bound.
use super::question::{
    graph_request_for, parse_graph_question, GraphQuestion, QuestionIntent, QuestionParse,
    MAX_QUESTION_BYTES, MAX_TOPIC_BYTES,
};
use crate::memory::personal_state::world::runtime_frame::GraphRequest;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};

pub(crate) const UNDERSTANDER_VERSION: &str = "g-query-1";
const MAX_CANDIDATES: usize = 5;
const MAX_RESPONSE_BYTES: usize = 2048;
const DEADLINE: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QueryUnderstanding {
    Requested(GraphQuestion),
    Ambiguous {
        candidates: Vec<String>,
        reason: String,
    },
    NotRequested,
    Unavailable {
        reason: String,
    },
}

impl QueryUnderstanding {
    pub(crate) fn graph_request(&self) -> Option<GraphRequest> {
        match self {
            Self::Requested(question) => Some(graph_request_for(question)),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelQuery {
    intent: String,
    topic: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    candidates: Vec<String>,
    #[serde(default)]
    confidence: f64,
}

pub(crate) fn understand(text: &str) -> QueryUnderstanding {
    understand_with_model(text, None)
}

pub(crate) fn understand_with_model(text: &str, model_json: Option<&str>) -> QueryUnderstanding {
    if text.len() > MAX_QUESTION_BYTES {
        return QueryUnderstanding::NotRequested;
    }
    match parse_graph_question(text) {
        QuestionParse::Requested(question) => return QueryUnderstanding::Requested(question),
        QuestionParse::Invalid => {
            return QueryUnderstanding::Unavailable {
                reason: "invalid_topic".into(),
            }
        }
        QuestionParse::NotRequested => {}
    }
    if greeting(text) {
        return QueryUnderstanding::NotRequested;
    }
    if let Some(parsed) = paraphrase(text) {
        return parsed;
    }
    if let Some(parsed) = english_paraphrase(text) {
        return parsed;
    }
    if let Some(json) = model_json {
        return accept_model_json(json);
    }
    QueryUnderstanding::NotRequested
}

pub(crate) fn accept_model_json(json: &str) -> QueryUnderstanding {
    if json.len() > MAX_RESPONSE_BYTES {
        return QueryUnderstanding::Unavailable {
            reason: "model_response_too_large".into(),
        };
    }
    let started = Instant::now();
    let parsed: Result<ModelQuery, _> = serde_json::from_str(json);
    if started.elapsed() > DEADLINE {
        return QueryUnderstanding::Unavailable {
            reason: "timeout".into(),
        };
    }
    let Ok(parsed) = parsed else {
        return QueryUnderstanding::Unavailable {
            reason: "malformed_json".into(),
        };
    };
    if parsed.intent.contains("select ")
        || parsed.topic.contains("scope:")
        || parsed.source.as_deref().unwrap_or("").contains("scope:")
    {
        return QueryUnderstanding::Unavailable {
            reason: "injected_scope".into(),
        };
    }
    let Some(intent) = parse_intent(&parsed.intent) else {
        return QueryUnderstanding::Unavailable {
            reason: "unknown_intent".into(),
        };
    };
    if parsed.candidates.len() > MAX_CANDIDATES {
        return QueryUnderstanding::Ambiguous {
            candidates: parsed.candidates.into_iter().take(MAX_CANDIDATES).collect(),
            reason: "too_many_candidates".into(),
        };
    }
    if parsed.topic.trim().is_empty() || parsed.topic.len() > MAX_TOPIC_BYTES {
        return QueryUnderstanding::Unavailable {
            reason: "invalid_topic".into(),
        };
    }
    if parsed.confidence < 0.5 && !parsed.candidates.is_empty() {
        return QueryUnderstanding::Ambiguous {
            candidates: parsed.candidates,
            reason: "low_confidence".into(),
        };
    }
    QueryUnderstanding::Requested(GraphQuestion {
        topic: parsed.topic,
        intent,
    })
}

pub(crate) fn digest_source(text: &str, scope: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{UNDERSTANDER_VERSION}:{scope}:{text}").as_bytes())
    )
}

fn parse_intent(value: &str) -> Option<QuestionIntent> {
    match value {
        "relevance" | "関係" => Some(QuestionIntent::Relevance),
        "influence" | "影響" => Some(QuestionIntent::Influence),
        "correlation" | "関連" | "相関" => Some(QuestionIntent::Correlation),
        "dependency" | "前提" | "依存" => Some(QuestionIntent::Dependency),
        _ => None,
    }
}

fn english_paraphrase(text: &str) -> Option<QueryUnderstanding> {
    let lower = text.to_lowercase();
    let intent = intent_from_text(&lower)?;
    let topic = if let Some(rest) = lower.strip_prefix("is ") {
        rest.split(" related").next().map(|v| v.trim().to_string())
    } else if let Some((_, rest)) = lower.split_once(" if ") {
        Some(
            rest.replace(" changed", "")
                .replace(" changes", "")
                .trim()
                .to_string(),
        )
    } else if let Some(rest) = lower.strip_prefix("how are ") {
        if !lower.contains("related") {
            return None;
        }
        Some(rest.replace(" related", "").trim().to_string())
    } else if let Some(rest) = lower.strip_prefix("what does ") {
        Some(
            rest.replace(" depend on", "")
                .replace(" depend", "")
                .trim()
                .to_string(),
        )
    } else {
        None
    }?;
    let topic = topic.trim_end_matches('?').trim().to_string();
    if topic.is_empty() {
        return None;
    }
    Some(QueryUnderstanding::Requested(GraphQuestion {
        topic,
        intent,
    }))
}

fn greeting(text: &str) -> bool {
    matches!(
        text.trim(),
        "こんにちは" | "おはよう" | "hello" | "hi" | "hey" | "お疲れ"
    )
}

fn paraphrase(text: &str) -> Option<QueryUnderstanding> {
    let trimmed = text.trim();
    let (topic, rest) = split_topic(trimmed)?;
    let intent = intent_from_text(&rest)?;
    if topic == "これ" || topic.eq_ignore_ascii_case("this") {
        return Some(QueryUnderstanding::Ambiguous {
            candidates: Vec::new(),
            reason: "unresolved_deictic".into(),
        });
    }
    Some(QueryUnderstanding::Requested(GraphQuestion {
        topic,
        intent,
    }))
}

fn split_topic(text: &str) -> Option<(String, String)> {
    let mut best = None;
    for marker in ["は", "が", "と", "の", "って"] {
        if let Some((topic, rest)) = text.split_once(marker) {
            let topic = topic
                .trim()
                .trim_matches('「')
                .trim_matches('」')
                .trim_matches('"')
                .trim();
            if topic.is_empty() || topic.len() > MAX_TOPIC_BYTES {
                continue;
            }
            let rest = rest.to_lowercase();
            if intent_from_text(&rest).is_some() {
                return Some((topic.to_string(), rest));
            }
            if best.is_none() {
                best = Some((topic.to_string(), rest));
            }
        }
    }
    for marker in [" about ", " of ", " between "] {
        if let Some((rest, topic)) = text.to_lowercase().split_once(marker) {
            let topic = topic
                .trim()
                .trim_end_matches('?')
                .trim_end_matches('？')
                .trim();
            if !topic.is_empty() {
                return Some((topic.to_string(), rest.to_string()));
            }
        }
    }
    best
}

fn intent_from_text(text: &str) -> Option<QuestionIntent> {
    let text = text.to_lowercase();
    if contains_any(
        &text,
        &[
            "関係ある",
            "関係あります",
            "今の目標",
            "どう関係",
            "relevant",
            "relate to the goal",
            "related to",
        ],
    ) {
        return Some(QuestionIntent::Relevance);
    }
    if contains_any(
        &text,
        &[
            "影響を受ける",
            "何が影響",
            "影響する",
            "変わると",
            "what is affected",
            "what would be affected",
            "influence",
        ],
    ) {
        return Some(QuestionIntent::Influence);
    }
    if contains_any(
        &text,
        &[
            "関連を知りたい",
            "関連",
            "相関",
            "correlation",
            "how are they related",
            "relationship between",
        ],
    ) || (text.contains("how are") && text.contains("related"))
    {
        return Some(QuestionIntent::Correlation);
    }
    if contains_any(
        &text,
        &[
            "前提は",
            "前提",
            "依存",
            "depend",
            "depends on",
            "what does it depend",
        ],
    ) {
        return Some(QuestionIntent::Dependency);
    }
    None
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

pub(crate) fn resolve_deictic(text: &str, focus: &[String]) -> QueryUnderstanding {
    let base = understand(text);
    match base {
        QueryUnderstanding::Ambiguous { reason, .. } if reason == "unresolved_deictic" => {
            match focus.len() {
                1 => QueryUnderstanding::Requested(GraphQuestion {
                    topic: focus[0].clone(),
                    intent: intent_from_text(&text.to_lowercase())
                        .unwrap_or(QuestionIntent::Relevance),
                }),
                n if n > 1 => QueryUnderstanding::Ambiguous {
                    candidates: focus.iter().take(MAX_CANDIDATES).cloned().collect(),
                    reason: "multiple_focus".into(),
                },
                _ => QueryUnderstanding::Unavailable {
                    reason: "unknown_topic".into(),
                },
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rf5_g_01_four_intents_and_greeting_is_not_requested() {
        assert!(matches!(
            understand("Xは今の目標に関係ある？"),
            QueryUnderstanding::Requested(question) if question.intent == QuestionIntent::Relevance
        ));
        assert!(matches!(
            understand("Xが変わると何が影響を受ける？"),
            QueryUnderstanding::Requested(question) if question.intent == QuestionIntent::Influence
        ));
        assert!(matches!(
            understand("XとYの関連を知りたい"),
            QueryUnderstanding::Requested(question) if question.intent == QuestionIntent::Correlation
        ));
        assert!(matches!(
            understand("Xの前提は？"),
            QueryUnderstanding::Requested(question) if question.intent == QuestionIntent::Dependency
        ));
        assert!(matches!(
            understand("こんにちは"),
            QueryUnderstanding::NotRequested
        ));
        assert!(matches!(
            understand("how are you"),
            QueryUnderstanding::NotRequested
        ));
    }

    #[test]
    fn rf5_g_02_host_rejects_malformed_unknown_and_injected_scope() {
        assert!(matches!(
            accept_model_json("{"),
            QueryUnderstanding::Unavailable { reason } if reason == "malformed_json"
        ));
        assert!(matches!(
            accept_model_json(r#"{"intent":"drop","topic":"x"}"#),
            QueryUnderstanding::Unavailable { reason } if reason == "unknown_intent"
        ));
        assert!(matches!(
            accept_model_json(r#"{"intent":"relevance","topic":"x","source":"scope:other"}"#),
            QueryUnderstanding::Unavailable { reason } if reason == "injected_scope"
        ));
    }

    #[test]
    fn rf5_g_03_deictic_resolution() {
        let unique = resolve_deictic(
            "これは今の目標に関係ある？",
            &["Speculative Decoding".into()],
        );
        assert!(
            matches!(unique, QueryUnderstanding::Requested(question) if question.topic == "Speculative Decoding")
        );
        let many = resolve_deictic("これの前提は？", &["a".into(), "b".into()]);
        assert!(matches!(many, QueryUnderstanding::Ambiguous { .. }));
    }
}
