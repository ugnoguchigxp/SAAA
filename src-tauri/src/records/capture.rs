use rusqlite::Connection;
use serde_json::{json, Value};

use super::auth::Authorization;
use super::outline::{self, OutlineKind};
use super::write::{commit, NewRecord};
use super::{Origin, RecordKind};

pub(crate) fn attach(
    connection: &Connection,
    auth: &Authorization,
    tool_name: &str,
    raw: &str,
) -> Result<String, String> {
    let value: Value = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    if kind == "web_search_result" {
        return attach_search(connection, auth, &value, raw);
    }
    if kind == "fetch_content_result" || tool_name == "fetch_content" {
        return attach_fetch(connection, auth, &value, raw);
    }
    Ok(raw.to_string())
}

fn attach_search(
    connection: &Connection,
    auth: &Authorization,
    value: &Value,
    raw: &str,
) -> Result<String, String> {
    let scopes = Vec::new();
    let search = commit(
        connection,
        NewRecord {
            kind: RecordKind::WebSearch,
            origin: Origin::ExternalObservation,
            principal_id: &auth.principal_id,
            conversation_id: &auth.conversation_id,
            run_id: None,
            turn_id: None,
            parent_execution_id: None,
            rank: None,
            observed_at: crate::schedule::tick::now_ms(),
            locator: json!({"query": value["query"]}),
            scope_keys: &scopes,
        },
        raw.as_bytes(),
        Some(raw),
    )?;
    let mut output = value.clone();
    output["searchRecordId"] = json!(search.id);
    if let Some(hits) = output.get_mut("hits").and_then(Value::as_array_mut) {
        for (index, hit) in hits.iter_mut().enumerate() {
            let body = hit.to_string();
            let stored = commit(
                connection,
                NewRecord {
                    kind: RecordKind::WebSearchResult,
                    origin: Origin::ExternalObservation,
                    principal_id: &auth.principal_id,
                    conversation_id: &auth.conversation_id,
                    run_id: None,
                    turn_id: None,
                    parent_execution_id: Some(&search.id),
                    rank: Some((index + 1) as u32),
                    observed_at: crate::schedule::tick::now_ms(),
                    locator: json!({"url": hit["url"]}),
                    scope_keys: &scopes,
                },
                body.as_bytes(),
                Some(&body),
            )?;
            hit["recordId"] = json!(stored.id);
            hit["searchRecordId"] = json!(search.id);
        }
    }
    Ok(output.to_string())
}

fn attach_fetch(
    connection: &Connection,
    auth: &Authorization,
    value: &Value,
    raw: &str,
) -> Result<String, String> {
    let text = value
        .pointer("/document/text")
        .or_else(|| value.get("text"))
        .and_then(Value::as_str)
        .unwrap_or(raw);
    let scopes = Vec::new();
    let stored = commit(
        connection,
        NewRecord {
            kind: RecordKind::WebFetch,
            origin: Origin::ExternalObservation,
            principal_id: &auth.principal_id,
            conversation_id: &auth.conversation_id,
            run_id: None,
            turn_id: None,
            parent_execution_id: None,
            rank: None,
            observed_at: crate::schedule::tick::now_ms(),
            locator: json!({"url": value["finalUrl"]}),
            scope_keys: &scopes,
        },
        text.as_bytes(),
        Some(text),
    )?;
    if text.len() <= 8_192 {
        let mut output = value.clone();
        output["recordId"] = json!(stored.id);
        return Ok(output.to_string());
    }
    let outline = outline::build(OutlineKind::Plain, text);
    Ok(json!({
        "type": "fetch_content_result",
        "recordId": stored.id,
        "bytes": text.len(),
        "sha256": stored.sha256,
        "outline": outline.items.iter().map(|item| json!({"start": item.start_byte, "text": item.text})).collect::<Vec<_>>(),
        "readHint": "read_record",
        "instruction_authority": "none",
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        crate::persistence::schema::initialize_database(&connection).unwrap();
        connection
    }

    fn auth() -> Authorization {
        Authorization {
            principal_id: "p".into(),
            conversation_id: "c".into(),
            allowed_scope_keys: vec![],
        }
    }

    #[test]
    fn cw_30_web_search_persists_execution_and_ranked_results() {
        let connection = db();
        let raw = json!({
            "type": "web_search_result",
            "query": "rust",
            "hits": [{"rank": 1, "title": "a", "url": "https://a.test"}, {"rank": 2, "title": "b", "url": "https://b.test"}]
        })
        .to_string();
        let output: Value = serde_json::from_str(&attach(&connection, &auth(), "web_search", &raw).unwrap()).unwrap();
        assert!(output["searchRecordId"].is_string());
        assert!(output["hits"][1]["recordId"].is_string());
        let ranks: i64 = connection
            .query_row(
                "SELECT count(*) FROM records WHERE kind='web_search_result' AND parent_execution_id=?1 AND rank IN (1,2)",
                [output["searchRecordId"].as_str().unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ranks, 2);
    }

    #[test]
    fn cw_30_store_failure_blocks_output() {
        let connection = db();
        connection.execute_batch("PRAGMA query_only=ON").unwrap();
        let raw = r#"{"type":"web_search_result","hits":[]}"#;
        let error = attach(&connection, &auth(), "web_search", raw).unwrap_err();
        assert!(!error.is_empty());
    }

    #[test]
    fn cw_31_fetch_over_8kib_returns_outline_not_body() {
        let connection = db();
        let text = "a".repeat(9_000);
        let raw = json!({"type":"fetch_content_result","document":{"text": text}}).to_string();
        let output: Value = serde_json::from_str(&attach(&connection, &auth(), "fetch_content", &raw).unwrap()).unwrap();
        assert_eq!(output["readHint"], "read_record");
        assert!(output.get("document").is_none());
    }

    #[test]
    fn cw_31_fetch_under_8kib_returns_body_with_record_id() {
        let connection = db();
        let raw = json!({"type":"fetch_content_result","document":{"text":"short"}}).to_string();
        let output: Value = serde_json::from_str(&attach(&connection, &auth(), "fetch_content", &raw).unwrap()).unwrap();
        assert_eq!(output["document"]["text"], "short");
        assert!(output["recordId"].is_string());
    }
}
