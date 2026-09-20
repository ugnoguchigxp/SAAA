#![allow(private_interfaces)]
//! Conversation/MCP gateway contract for the three fixed entry points. Names stay
//! `tools_search`/`tools_describe`/`tools_invoke` for provider compatibility; internally they map
//! to `tools.search` etc. Every response is a fixed `{ok,data}` / `{ok,error}` envelope and no
//! internal error detail is ever returned to the model.

use serde_json::{json, Value};

use super::contracts::*;
use super::service::{self, ToolSelectionService};
use crate::runtime::agent_tools::AgentToolCall;
use crate::{AppState, RunCancellation};

pub const TOOL_SEARCH: &str = "tools_search";
pub const TOOL_DESCRIBE: &str = "tools_describe";
pub const TOOL_INVOKE: &str = "tools_invoke";

pub fn internal_name(name: &str) -> Option<&'static str> {
    match name {
        TOOL_SEARCH => Some("tools.search"),
        TOOL_DESCRIBE => Some("tools.describe"),
        TOOL_INVOKE => Some("tools.invoke"),
        _ => None,
    }
}

pub fn is_selection_tool(name: &str) -> bool {
    internal_name(name).is_some()
}

pub fn search_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "intent": { "type": "string" },
            "limit": { "type": "integer", "minimum": 1, "maximum": 8 }
        },
        "required": ["intent"],
        "additionalProperties": false
    })
}

pub fn describe_schema() -> Value {
    json!({
        "type": "object",
        "oneOf": [
            {
                "properties": {
                    "candidateRef": { "type": "string" },
                    "section": {
                        "type": "string",
                        "enum": ["contract", "usage", "examples", "troubleshooting"]
                    },
                    "cursor": { "type": "string" }
                },
                "required": ["candidateRef"],
                "additionalProperties": false
            },
            {
                "properties": {
                    "resultRef": { "type": "string" },
                    "page": { "type": "integer", "minimum": 0 }
                },
                "required": ["resultRef", "page"],
                "additionalProperties": false
            }
        ]
    })
}

pub fn invoke_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "executionRef": { "type": "string" },
            "arguments": { "type": "object" }
        },
        "required": ["executionRef", "arguments"],
        "additionalProperties": false
    })
}

/// OpenAI Chat Completions tool definitions. The MCP layer (D4/D5) reuses the same neutral
/// schemas and only changes the transport wrapper.
pub use super::gateway_schemas::definitions;

pub fn ok(data: Value) -> Value {
    json!({ "ok": true, "data": data })
}

pub fn error_envelope(error: &ToolSelectionError) -> Value {
    json!({
        "ok": false,
        "error": {
            "code": error.code.as_str(),
            "message": error.message,
            "retryable": error.retryable,
        }
    })
}

/// Routes one tool call from the conversation provider. `arguments` is the raw JSON string from
/// the provider; every field is validated here before the service is reached.
pub async fn dispatch(
    service: &ToolSelectionService,
    context: &RequestContext,
    name: &str,
    arguments: &str,
    cancellation: &RunCancellation,
) -> Value {
    if arguments.len() > GATEWAY_INPUT_MAX_BYTES {
        return error_envelope(&ToolSelectionError::invalid());
    }
    let parsed: Value = match serde_json::from_str(arguments) {
        Ok(value) => value,
        Err(_) => return error_envelope(&ToolSelectionError::invalid()),
    };
    let result = match internal_name(name) {
        Some("tools.search") => dispatch_search(service, context, &parsed, None).await,
        Some("tools.describe") => dispatch_describe(service, context, &parsed),
        Some("tools.invoke") => dispatch_invoke(service, context, &parsed, cancellation).await,
        _ => Err(ToolSelectionError::invalid()),
    };
    match result {
        Ok(data) => ok(data),
        Err(error) => error_envelope(&error),
    }
}

/// MCP-facing dispatch. It shares the three entry points' validation and schemas with the
/// conversation path, but a search uses a request-local scenario extracted from the intent and
/// never persists the intent as user feedback.
pub async fn dispatch_external(
    service: &ToolSelectionService,
    context: &RequestContext,
    name: &str,
    arguments: &str,
    cancellation: &RunCancellation,
) -> Value {
    if arguments.len() > GATEWAY_INPUT_MAX_BYTES {
        return error_envelope(&ToolSelectionError::invalid());
    }
    let parsed: Value = match serde_json::from_str(arguments) {
        Ok(value) => value,
        Err(_) => return error_envelope(&ToolSelectionError::invalid()),
    };
    let result = match internal_name(name) {
        Some("tools.search") => {
            let intent = match parsed.get("intent").and_then(Value::as_str) {
                Some(intent) => intent,
                None => return error_envelope(&ToolSelectionError::invalid()),
            };
            let scenario = service.extract_scenario_only(context, intent).await;
            dispatch_search(service, context, &parsed, Some(&scenario)).await
        }
        Some("tools.describe") => dispatch_describe(service, context, &parsed),
        Some("tools.invoke") => dispatch_invoke(service, context, &parsed, cancellation).await,
        _ => Err(ToolSelectionError::invalid()),
    };
    match result {
        Ok(data) => ok(data),
        Err(error) => error_envelope(&error),
    }
}

async fn dispatch_search(
    service: &ToolSelectionService,
    context: &RequestContext,
    arguments: &Value,
    scenario: Option<&Scenario>,
) -> ToolSelectionResult<Value> {
    let object = arguments
        .as_object()
        .ok_or_else(ToolSelectionError::invalid)?;
    if object.keys().any(|key| key != "intent" && key != "limit") {
        return Err(ToolSelectionError::invalid());
    }
    let intent = object
        .get("intent")
        .and_then(Value::as_str)
        .ok_or_else(ToolSelectionError::invalid)?;
    let limit = match object.get("limit") {
        None => SEARCH_LIMIT_DEFAULT,
        Some(value) => value
            .as_u64()
            .filter(|limit| (1..=SEARCH_LIMIT_MAX as u64).contains(limit))
            .ok_or_else(ToolSelectionError::invalid)? as usize,
    };
    let response = match scenario {
        Some(scenario) => {
            service
                .search_with_scenario(context, intent, limit, scenario)
                .await?
        }
        None => service.search(context, intent, limit).await?,
    };
    let candidates: Vec<Value> = response
        .candidates
        .iter()
        .map(|candidate| {
            json!({
                "candidateRef": candidate.reference,
                "toolId": candidate.tool_id,
                "sourceId": candidate.source_id,
                "sourceLabel": candidate.source_label,
                "title": candidate.title,
                "summary": candidate.summary,
                "reason": candidate.reason,
            })
        })
        .collect();
    let data = json!({
        "decisionId": response.decision_id,
        "status": response.status.as_str(),
        "candidates": candidates,
    });
    let bytes = serde_json::to_vec(&data).map_err(|_| ToolSelectionError::storage())?;
    if bytes.len() > SEARCH_RESPONSE_MAX_BYTES {
        return Err(ToolSelectionError::new(
            ToolSelectionErrorCode::Unavailable,
            "The search result exceeded the local limit.",
        ));
    }
    Ok(data)
}

fn dispatch_describe(
    service: &ToolSelectionService,
    context: &RequestContext,
    arguments: &Value,
) -> ToolSelectionResult<Value> {
    let object = arguments
        .as_object()
        .ok_or_else(ToolSelectionError::invalid)?;
    let known = ["candidateRef", "section", "cursor", "resultRef", "page"];
    if object.keys().any(|key| !known.contains(&key.as_str())) {
        return Err(ToolSelectionError::invalid());
    }
    // The two branches are mutually exclusive, matching the oneOf schema.
    let result_branch = object.contains_key("resultRef") || object.contains_key("page");
    if result_branch {
        if object.contains_key("candidateRef")
            || object.contains_key("section")
            || object.contains_key("cursor")
        {
            return Err(ToolSelectionError::invalid());
        }
        let result_ref = object
            .get("resultRef")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(ToolSelectionError::invalid)?;
        let page = object
            .get("page")
            .and_then(Value::as_i64)
            .filter(|page| *page >= 0)
            .ok_or_else(ToolSelectionError::invalid)?;
        let response = service.describe_result(context, result_ref, page)?;
        let data = json!({
            "resultRef": response.result_ref,
            "page": response.page,
            "pageCount": response.page_count,
            "encoding": "json-text",
            "text": response.text,
        });
        return bounded_describe(data);
    }
    let candidate_ref = object
        .get("candidateRef")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(ToolSelectionError::invalid)?;
    let section = match object.get("section") {
        None => "contract",
        Some(Value::String(value)) => value.as_str(),
        Some(_) => return Err(ToolSelectionError::invalid()),
    };
    let cursor = match object.get("cursor") {
        None => None,
        Some(Value::String(value)) => Some(value.as_str()),
        Some(_) => return Err(ToolSelectionError::invalid()),
    };
    let response = service.describe(context, candidate_ref, section, cursor)?;
    let data = json!({
        "revisionId": response.revision_id,
        "toolId": response.tool_id,
        "sourceId": response.source_id,
        "sourceLabel": response.source_label,
        "section": response.section,
        "body": response.body,
        "executionRef": response.execution_ref,
        "cursor": response.cursor,
    });
    bounded_describe(data)
}

fn bounded_describe(data: Value) -> ToolSelectionResult<Value> {
    let bytes = serde_json::to_vec(&data).map_err(|_| ToolSelectionError::storage())?;
    if bytes.len() > DESCRIBE_RESPONSE_MAX_BYTES {
        return Err(ToolSelectionError::new(
            ToolSelectionErrorCode::Unavailable,
            "The tool description exceeded the local limit.",
        ));
    }
    Ok(data)
}

async fn dispatch_invoke(
    service: &ToolSelectionService,
    context: &RequestContext,
    arguments: &Value,
    cancellation: &RunCancellation,
) -> ToolSelectionResult<Value> {
    let object = arguments
        .as_object()
        .ok_or_else(ToolSelectionError::invalid)?;
    if object
        .keys()
        .any(|key| key != "executionRef" && key != "arguments")
    {
        return Err(ToolSelectionError::invalid());
    }
    let execution_ref = object
        .get("executionRef")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(ToolSelectionError::invalid)?;
    let call_arguments = object
        .get("arguments")
        .filter(|value| value.is_object())
        .ok_or_else(ToolSelectionError::invalid)?;
    let response = service
        .invoke(context, execution_ref, call_arguments, cancellation)
        .await?;
    Ok(json!({
        "invocationId": response.invocation_id,
        "status": response.status.as_str(),
        "result": response.result,
        "resultRef": response.result_ref,
        "byteCount": response.byte_count,
        "pageCount": response.page_count,
        "resultAvailability": response.result_availability,
        "error": response.error_code.map(|code| json!({ "code": code })),
    }))
}

/// Appends the three selection definitions when discovery is enabled for this request.
pub fn append_tool_definitions(
    target: &mut Vec<Value>,
    persistence: Option<crate::ProviderOutputPersistence<'_>>,
) {
    if let Some(persistence) = persistence {
        if persistence.state.tool_selection.discovery_configured() {
            target.extend(definitions());
        }
    }
}

/// Conversation integration entry used by the provider dispatch. It owns the host-confirmed
/// context (principal/run/message) and never trusts model-supplied scope.
pub async fn execute_for_persistence(
    persistence: Option<crate::ProviderOutputPersistence<'_>>,
    input: &crate::StartTurnInput,
    call: &AgentToolCall,
    cancellation: &RunCancellation,
) -> String {
    // Link decisions to the persisted user message for this run, not only to a retry id.
    let input_message_id = input.retry_input_message_id.clone().or_else(|| {
        persistence.and_then(|persistence| {
            persistence
                .state
                .sqlite_readers
                .read(|connection| {
                    connection
                        .query_row(
                            "SELECT input_message_id FROM runtime_runs WHERE id = ?1",
                            rusqlite::params![input.run_id],
                            |row| row.get::<_, Option<String>>(0),
                        )
                        .map_err(|error| error.to_string())
                })
                .ok()
                .flatten()
        })
    });
    execute_for_turn(
        persistence.map(|persistence| persistence.state),
        &input.conversation_id,
        &input.run_id,
        input_message_id,
        call,
        cancellation,
    )
    .await
}

pub async fn execute_for_turn(
    state: Option<&AppState>,
    conversation_id: &str,
    run_id: &str,
    input_message_id: Option<String>,
    call: &AgentToolCall,
    cancellation: &RunCancellation,
) -> String {
    let Some(state) = state else {
        return crate::runtime::agent_tools::tool_error_content(
            "unavailable",
            "Tool selection is temporarily unavailable.",
        );
    };
    let Ok(principal) = service::ensure_principal(&state.sqlite_writer) else {
        return crate::runtime::agent_tools::tool_error_content(
            "unavailable",
            "Tool selection is temporarily unavailable.",
        );
    };
    let context = RequestContext::new(&principal, conversation_id)
        .with_run(Some(run_id.to_string()))
        .with_message(input_message_id);
    dispatch(
        &state.tool_selection,
        &context,
        &call.name,
        &call.arguments,
        cancellation,
    )
    .await
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_entry_points_are_fixed() {
        let definitions = definitions();
        let names: Vec<&str> = definitions
            .iter()
            .filter_map(|definition| definition.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, vec![TOOL_SEARCH, TOOL_DESCRIBE, TOOL_INVOKE]);
        assert_eq!(internal_name(TOOL_SEARCH), Some("tools.search"));
        assert!(is_selection_tool(TOOL_INVOKE));
        assert!(!is_selection_tool("recall"));
    }

    #[test]
    fn search_schema_is_the_fixed_contract() {
        let schema = search_schema();
        assert_eq!(schema.pointer("/properties/limit/maximum"), Some(&json!(8)));
        assert_eq!(schema.pointer("/properties/limit/minimum"), Some(&json!(1)));
        assert_eq!(schema.pointer("/additionalProperties"), Some(&json!(false)));
        assert_eq!(schema.pointer("/required/0"), Some(&json!("intent")));
    }

    #[test]
    fn error_envelope_is_stable() {
        let envelope = error_envelope(&ToolSelectionError::invalid());
        assert_eq!(envelope.pointer("/ok"), Some(&json!(false)));
        assert_eq!(
            envelope.pointer("/error/code"),
            Some(&json!("invalid-input"))
        );
        assert_eq!(envelope.pointer("/error/retryable"), Some(&json!(false)));
    }
}
