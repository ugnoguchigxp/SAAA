use super::*;
pub(crate) fn compact_result(tool_name: &str, result: &Value) -> Result<String, SearchError> {
    let content = result
        .get("content")
        .and_then(Value::as_array)
        .filter(|content| content.len() == 1)
        .and_then(|content| content.first())
        .filter(|content| content.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(|content| content.get("text").and_then(Value::as_str))
        .ok_or(SearchError::InvalidResponse)?;
    if content.len() > MAX_RESULT_BYTES {
        return Err(SearchError::ResponseTooLarge);
    }
    let payload: Value = serde_json::from_str(content).map_err(|_| SearchError::InvalidResponse)?;
    let payload = if tool_name == "fetch_episode" && payload.get("id").is_some() {
        json!({"items":[payload]})
    } else {
        payload
    };
    let items = payload
        .get("items")
        .and_then(Value::as_array)
        .ok_or(SearchError::InvalidResponse)?;
    let projected = items
        .iter()
        .take(MAX_ITEMS)
        .map(|item| compact_item(tool_name, item))
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_string(&json!({
        "trust": {"trustClass":"untrusted_memory_evidence","instructionAuthority":"none"},
        "source": "context_still",
        "memoryType": if tool_name == SEARCH_KNOWLEDGE_TOOL_NAME {"knowledge"} else {"episode"},
        "items": projected,
        "noContent": projected.is_empty(),
        "truncated": items.len() > MAX_ITEMS
    }))
    .map_err(|_| SearchError::InvalidResponse)
}
pub(super) fn compact_item(tool_name: &str, item: &Value) -> Result<Value, SearchError> {
    let object = item.as_object().ok_or(SearchError::InvalidResponse)?;
    let fields: &[&str] = if tool_name == SEARCH_KNOWLEDGE_TOOL_NAME {
        &["title", "body", "type", "polarity", "score", "scope"]
    } else {
        &[
            "title",
            "situation",
            "outcome",
            "lesson",
            "outcomeKind",
            "score",
            "scope",
        ]
    };
    let mut projected = Map::new();
    for field in fields {
        if let Some(value) = object.get(*field) {
            let value = match value {
                Value::String(text) => Value::String(text.chars().take(4_000).collect()),
                Value::Number(_) | Value::Bool(_) | Value::Null => value.clone(),
                _ => continue,
            };
            projected.insert((*field).to_string(), value);
        }
    }
    if object.get("sourceContract").is_some_and(Value::is_object) {
        for key in ["id", "sourceKey", "sourceContract", "eventTime"] {
            if let Some(v) = object.get(key) {
                projected.insert(key.into(), v.clone());
            }
        }
        for key in ["observations", "action"] {
            if let Some(v) = object.get(key).and_then(Value::as_str) {
                projected.insert(key.into(), Value::String(v.chars().take(4000).collect()));
            }
        }
    }
    Ok(Value::Object(projected))
}
