//! Bounded web provenance and audit metadata.
use super::*;
pub(super) fn web_result_urls(found: &str, kind: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(found) else {
        return Vec::new();
    };
    let candidates: Vec<&str> = match kind {
        "hits" => value["hits"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|hit| hit["url"].as_str())
            .collect(),
        "document" => value
            .pointer("/document/url")
            .and_then(Value::as_str)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    };
    candidates
        .into_iter()
        .filter(|raw| {
            url::Url::parse(raw).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            })
        })
        .take(5)
        .map(str::to_string)
        .collect()
}

pub(super) fn audit_web_tool_result(
    audit: &ConversationAudit,
    name: &str,
    step: usize,
    found: &str,
) {
    let parsed = serde_json::from_str::<Value>(found).ok();
    let error_code = parsed
        .as_ref()
        .and_then(|value| value.pointer("/error/code"))
        .and_then(Value::as_str);
    let hit_count = parsed
        .as_ref()
        .and_then(|value| value.get("hits"))
        .and_then(Value::as_array)
        .map(Vec::len);
    let retrieval_status = parsed
        .as_ref()
        .and_then(|value| value.pointer("/document/retrievalStatus"))
        .and_then(Value::as_str);
    audit.event(
        "provider",
        "conversation-web-tool-result",
        "terminal",
        Some(if error_code.is_some() {
            "failure"
        } else {
            "success"
        }),
        json!({"tool":name,"step":step,"resultBytes":found.len(),"errorCode":error_code,
            "hitCount":hit_count,"retrievalStatus":retrieval_status}),
    );
}
