#![allow(private_interfaces)]
//! Conversation/MCP gateway contract for the three fixed entry points. Names stay
//! `tools_search`/`tools_describe`/`tools_invoke` for provider compatibility; internally they map
//! to `tools.search` etc. Every response is a fixed `{ok,data}` / `{ok,error}` envelope and no
//! internal error detail is ever returned to the model.

use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::contracts::*;
use super::service::{self, ToolSelectionService};
use crate::persistence::SqliteWriter;
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
        Some("tools.invoke") => {
            dispatch_invoke(service, context, &parsed, cancellation, "conversation").await
        }
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
            // Validate the shared search shape before spending a provider call on scenario
            // extraction, so malformed external requests cannot drive the extractor.
            let (intent, _) = match search_arguments(&parsed) {
                Ok(parsed) => parsed,
                Err(error) => return error_envelope(&error),
            };
            let scenario = service.extract_scenario_only(context, intent).await;
            dispatch_search(service, context, &parsed, Some(&scenario)).await
        }
        Some("tools.describe") => dispatch_describe(service, context, &parsed),
        Some("tools.invoke") => {
            dispatch_invoke(service, context, &parsed, cancellation, "mcp").await
        }
        _ => Err(ToolSelectionError::invalid()),
    };
    match result {
        Ok(data) => ok(data),
        Err(error) => error_envelope(&error),
    }
}

/// Validates the shared search arguments without running retrieval or the scenario extractor.
fn search_arguments(arguments: &Value) -> ToolSelectionResult<(&str, usize)> {
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
    if intent.trim().is_empty() || intent.len() > SEARCH_INTENT_MAX_BYTES {
        return Err(ToolSelectionError::invalid());
    }
    let limit = match object.get("limit") {
        None => SEARCH_LIMIT_DEFAULT,
        Some(value) => value
            .as_u64()
            .filter(|limit| (1..=SEARCH_LIMIT_MAX as u64).contains(limit))
            .ok_or_else(ToolSelectionError::invalid)? as usize,
    };
    Ok((intent, limit))
}

async fn dispatch_search(
    service: &ToolSelectionService,
    context: &RequestContext,
    arguments: &Value,
    scenario: Option<&Scenario>,
) -> ToolSelectionResult<Value> {
    let (intent, limit) = search_arguments(arguments)?;
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
    origin: &'static str,
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
        .invoke_with_origin(context, execution_ref, call_arguments, cancellation, origin)
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
    execute_for_root(
        &state.tool_selection,
        &state.sqlite_writer,
        conversation_id,
        run_id,
        input_message_id,
        &call.name,
        &call.arguments,
        cancellation,
        false,
    )
    .await
    .to_string()
}

/// Executes a role-routed tool call through the existing gateway while keeping the operation
/// receipt owned by the role root. This is also used by the restricted MCP bridge; the bridge
/// never obtains a second, session-local routing id.
pub async fn execute_for_role_root(
    service: &ToolSelectionService,
    writer: &SqliteWriter,
    conversation_id: &str,
    root_id: &str,
    input_message_id: Option<String>,
    name: &str,
    arguments: &str,
    cancellation: &RunCancellation,
) -> Value {
    execute_for_root(
        service,
        writer,
        conversation_id,
        root_id,
        input_message_id,
        name,
        arguments,
        cancellation,
        true,
    )
    .await
}

async fn execute_for_root(
    service: &ToolSelectionService,
    writer: &SqliteWriter,
    conversation_id: &str,
    root_id: &str,
    input_message_id: Option<String>,
    name: &str,
    arguments: &str,
    cancellation: &RunCancellation,
    external: bool,
) -> Value {
    let Ok(principal) = service::ensure_principal(writer) else {
        return json!({"error":{"code":"unavailable","message":"Tool selection is temporarily unavailable."}});
    };
    let context = RequestContext::new(&principal, conversation_id)
        .with_run(Some(root_id.to_string()))
        .with_message(input_message_id);
    let operation_key = format!(
        "{:x}",
        Sha256::digest(format!("{name}:{arguments}").as_bytes())
    );
    // Enforce the role permit from the trusted tool effect before anything reaches the owner. A
    // reviewer can never reach a mutating tool, and an unpublished/unknown tool is fail-closed.
    if let Err(reason) = authorize_routing_tool(writer, root_id, name) {
        return json!({"error":{"code":"role-tool-denied","message":format!("Role-routing tool permit denied: {reason}")}});
    }
    match reserve_routing_operation(writer, root_id, &operation_key) {
        Ok(true) => {}
        Ok(false) => {
            return json!({"error":{"code":"operation-not-retryable","message":"This routing tool operation is already owned by an earlier invocation."}});
        }
        Err(_) => {
            return json!({"error":{"code":"unavailable","message":"Tool routing persistence is temporarily unavailable."}});
        }
    }
    let output = if external {
        dispatch_external(service, &context, name, arguments, cancellation).await
    } else {
        dispatch(service, &context, name, arguments, cancellation).await
    };
    settle_routing_operation(writer, root_id, &operation_key, &output);
    output
}

/// Maps the active routing step purpose to its role and checks the trusted tool effect. Steps that
/// do not belong to a routing root (or a conversation without a running step) keep the legacy
/// unrestricted meaning because this gateway only routes role-root calls.
fn authorize_routing_tool(writer: &SqliteWriter, root_id: &str, name: &str) -> Result<(), String> {
    let root_id = root_id.to_string();
    let name = name.to_string();
    writer.read_serialized(move |connection| {
        let purpose: Option<String> = connection
            .query_row(
                "SELECT purpose FROM rr_steps WHERE root_id=?1 AND status IN ('running','draining') ORDER BY ordinal LIMIT 1",
                [&root_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some(purpose) = purpose else {
            return Ok(());
        };
        let role = match purpose.as_str() {
            "review" => "reviewer",
            "respond" | "reconsider" | "revise" => "reasoner",
            "tool_specialist" => "tool_specialist",
            "frontend" => "frontend",
            other => return Err(format!("Role-routing step purpose {other} has no tool role")),
        };
        let effect = crate::tool_selection::repository::effect_for_backend_key(connection, &name)?;
        crate::role_routing::tools::permits_effect(
            role,
            crate::role_routing::tools::classify_effect(effect.as_deref()),
        )
        .map_err(str::to_string)
    })
}

/// The existing tool-selection service remains the invocation owner.  Role routing only reserves
/// its operation key before dispatch and records the owner's receipt afterwards.  A process that
/// dies after `dispatched` therefore cannot silently replay a potentially mutating operation.
fn reserve_routing_operation(
    writer: &SqliteWriter,
    root_id: &str,
    operation_key: &str,
) -> Result<bool, String> {
    let root_id = root_id.to_string();
    let operation_key = operation_key.to_string();
    writer.write(move |connection| {
        let transaction = connection.transaction().map_err(|error| error.to_string())?;
        let step: Option<(String, u32)> = transaction
            .query_row(
                "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status='running' ORDER BY ordinal LIMIT 1",
                [&root_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let step = match step {
            Some(step) => Some(step),
            // Before the coordinator claims a step, fall back to the lowest planned step so the
            // reservation still binds to the step the result will belong to.
            None => transaction
                .query_row(
                    "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status IN ('planned','draining') ORDER BY ordinal LIMIT 1",
                    [&root_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| error.to_string())?,
        };
        let Some((step_id, revision)) = step else {
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(true);
        };
        if crate::role_routing::tool_ledger::find_by_operation(&transaction, &root_id, &operation_key)?.is_some() {
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(false);
        }
        // The link id must include the root so the same name+arguments used by two different roots
        // cannot collide on the primary key.
        let link_id = format!(
            "rr-tool-{:x}",
            Sha256::digest(format!("{root_id}:{operation_key}").as_bytes())
        );
        let link = crate::role_routing::tool_ledger::ToolLink {
            id: link_id.chars().take(32).collect(),
            root_id: root_id.clone(),
            step_id,
            revision,
            operation_key: operation_key.clone(),
            invocation_id: None,
            dispatch_state: "reserved".into(),
            result_ref: None,
        };
        let now_ms = now_ms();
        crate::role_routing::tool_ledger::reserve(&transaction, &link, now_ms)?;
        crate::role_routing::tool_ledger::settle(
            &transaction,
            &root_id,
            &operation_key,
            None,
            None,
            "dispatched",
            now_ms,
        )?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(true)
    })
}

fn settle_routing_operation(
    writer: &SqliteWriter,
    root_id: &str,
    operation_key: &str,
    output: &Value,
) {
    let invocation_id = output
        .pointer("/data/invocationId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let result_ref = output
        .pointer("/data/resultRef")
        .and_then(Value::as_str)
        .map(str::to_owned);
    // A transport/unavailable failure leaves the remote outcome unknown; it must not be recorded
    // as a settled success, because that would license a later retry of a possibly applied
    // mutation. Local validation failures are terminal and safe to settle.
    let error_code = output.pointer("/error/code").and_then(Value::as_str);
    let unknown = matches!(error_code, Some("unavailable" | "timeout" | "transport"));
    let state = if unknown { "unknown" } else { "settled" };
    let root_id = root_id.to_string();
    let operation_key = operation_key.to_string();
    let _ = writer.write(move |connection| {
        crate::role_routing::tool_ledger::settle(
            connection,
            &root_id,
            &operation_key,
            invocation_id.as_deref(),
            result_ref.as_deref(),
            state,
            now_ms(),
        )
        .map(|_| ())
    });
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
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
