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
        None,
    )
    .await
    .to_string()
}

/// Executes a role-routed tool call through the existing gateway while keeping the operation
/// receipt owned by the role root. This is also used by the restricted MCP bridge; the bridge
/// never obtains a second, session-local routing id.
pub struct RoleStepBinding<'a> {
    pub root_id: &'a str,
    pub step_id: &'a str,
    pub revision: i64,
    pub attempt_started_at_ms: i64,
    pub config_fingerprint: &'a str,
}

#[allow(clippy::too_many_arguments)] // Role-root authority and cancellation inputs remain explicit.
pub async fn execute_for_role_root(
    service: &ToolSelectionService,
    writer: &SqliteWriter,
    conversation_id: &str,
    binding: &RoleStepBinding<'_>,
    input_message_id: Option<String>,
    name: &str,
    arguments: &str,
    cancellation: &RunCancellation,
) -> Value {
    execute_for_root(
        service,
        writer,
        conversation_id,
        binding.root_id,
        input_message_id,
        name,
        arguments,
        cancellation,
        true,
        Some(binding),
    )
    .await
}

#[allow(clippy::too_many_arguments)] // Shared gateway boundary carries audited execution context.
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
    role_binding: Option<&RoleStepBinding<'_>>,
) -> Value {
    let Ok(principal) = service::ensure_principal(writer) else {
        return json!({"error":{"code":"unavailable","message":"Tool selection is temporarily unavailable."}});
    };
    let context = RequestContext::new(&principal, conversation_id)
        .with_run(Some(root_id.to_string()))
        .with_message(input_message_id);
    let operation_key = routing_operation_key(name, arguments);
    // Enforce the role permit from the trusted tool effect before anything reaches the owner. A
    // reviewer can never reach a mutating tool, and an unpublished/unknown tool is fail-closed.
    let resolved_effect = resolve_role_tool_effect(service, &context, name, arguments);
    if let Err(reason) = authorize_routing_tool(writer, root_id, resolved_effect, role_binding) {
        return json!({"error":{"code":"role-tool-denied","message":format!("Role-routing tool permit denied: {reason}")}});
    }
    match reserve_routing_operation(writer, root_id, &operation_key, role_binding) {
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
    if settle_routing_operation(writer, root_id, &operation_key, &output).is_err() {
        return json!({"error":{"code":"routing-settle-failed","message":"The tool owner finished, but its routing receipt could not be settled. Do not retry automatically."}});
    }
    output
}

fn routing_operation_key(name: &str, arguments: &str) -> String {
    let canonical = serde_json::from_str::<Value>(arguments)
        .map(|value| crate::tool_selection::catalog::canonical_json(&value))
        .unwrap_or_else(|_| arguments.to_string());
    format!(
        "{:x}",
        Sha256::digest(format!("{name}\0{canonical}").as_bytes())
    )
}

/// Maps the active routing step purpose to its role and checks the trusted tool effect. Steps that
/// do not belong to a routing root (or a conversation without a running step) keep the legacy
/// unrestricted meaning because this gateway only routes role-root calls.
fn authorize_routing_tool(
    writer: &SqliteWriter,
    root_id: &str,
    effect: crate::role_routing::tools::ToolEffect,
    binding: Option<&RoleStepBinding<'_>>,
) -> Result<(), String> {
    let root_id = root_id.to_string();
    let binding = binding.map(|binding| {
        (
            binding.step_id.to_string(),
            binding.revision,
            binding.attempt_started_at_ms,
            binding.config_fingerprint.to_string(),
        )
    });
    writer.read_serialized(move |connection| {
        let purpose: Option<String> = if let Some((step_id, revision, started_at, fingerprint)) = &binding {
            connection
            .query_row(
                "SELECT s.purpose FROM rr_steps s JOIN rr_roots r ON r.root_id=s.root_id
                 WHERE s.root_id=?1 AND s.id=?2 AND s.revision=?3 AND s.started_at_ms=?4
                   AND s.config_fingerprint=?5 AND s.status='running' AND r.phase='responding'
                   AND r.revision=s.revision AND r.cancel_requested=0",
                rusqlite::params![&root_id, step_id, revision, started_at, fingerprint],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
        } else {
            connection
                .query_row(
                    "SELECT purpose FROM rr_steps WHERE root_id=?1 AND status IN ('running','draining') ORDER BY ordinal LIMIT 1",
                    [&root_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?
        };
        let Some(purpose) = purpose else {
            return if binding.is_some() {
                Err("routing MCP session binding is stale".into())
            } else {
                Ok(())
            };
        };
        let role = match purpose.as_str() {
            "review" => "reviewer",
            "respond" | "reconsider" | "revise" => "reasoner",
            "tool_specialist" => "tool_specialist",
            "frontend" => "frontend",
            other => return Err(format!("Role-routing step purpose {other} has no tool role")),
        };
        crate::role_routing::tools::permits_effect(role, effect).map_err(str::to_string)
    })
}

fn resolve_role_tool_effect(
    service: &ToolSelectionService,
    context: &RequestContext,
    name: &str,
    arguments: &str,
) -> crate::role_routing::tools::ToolEffect {
    match name {
        TOOL_SEARCH | "tools.search" | TOOL_DESCRIBE | "tools.describe" => {
            crate::role_routing::tools::ToolEffect::ReadOnly
        }
        TOOL_INVOKE | "tools.invoke" => serde_json::from_str::<Value>(arguments)
            .ok()
            .and_then(|arguments| {
                arguments
                    .get("executionRef")
                    .and_then(Value::as_str)
                    .and_then(|reference| service.execution_effect(context, reference).ok())
            })
            .map(|effect| crate::role_routing::tools::classify_effect(Some(&effect)))
            .unwrap_or(crate::role_routing::tools::ToolEffect::Mutating),
        _ => crate::role_routing::tools::ToolEffect::Mutating,
    }
}

/// The existing tool-selection service remains the invocation owner.  Role routing only reserves
/// its operation key before dispatch and records the owner's receipt afterwards.  A process that
/// dies after `dispatched` therefore cannot silently replay a potentially mutating operation.
fn reserve_routing_operation(
    writer: &SqliteWriter,
    root_id: &str,
    operation_key: &str,
    binding: Option<&RoleStepBinding<'_>>,
) -> Result<bool, String> {
    let root_id = root_id.to_string();
    let operation_key = operation_key.to_string();
    let binding = binding.map(|binding| {
        (
            binding.step_id.to_string(),
            binding.revision,
            binding.attempt_started_at_ms,
            binding.config_fingerprint.to_string(),
        )
    });
    writer.write(move |connection| {
        let transaction = connection.transaction().map_err(|error| error.to_string())?;
        let step: Option<(String, u32)> = if let Some((step_id, revision, started_at, fingerprint)) = &binding {
            transaction.query_row(
                "SELECT s.id,s.revision FROM rr_steps s JOIN rr_roots r ON r.root_id=s.root_id
                 WHERE s.root_id=?1 AND s.id=?2 AND s.revision=?3 AND s.started_at_ms=?4
                   AND s.config_fingerprint=?5 AND s.status='running' AND r.phase='responding'
                   AND r.revision=s.revision AND r.cancel_requested=0",
                rusqlite::params![&root_id, step_id, revision, started_at, fingerprint],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().map_err(|error| error.to_string())?
        } else {
            transaction
                .query_row(
                    "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status='running' ORDER BY ordinal LIMIT 1",
                    [&root_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| error.to_string())?
        };
        let step = match step {
            Some(step) => Some(step),
            // Before the coordinator claims a step, fall back to the lowest planned step so the
            // reservation still binds to the step the result will belong to.
            None if binding.is_none() => transaction
                .query_row(
                    "SELECT id,revision FROM rr_steps WHERE root_id=?1 AND status IN ('planned','draining') ORDER BY ordinal LIMIT 1",
                    [&root_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| error.to_string())?,
            None => return Err("Role-routing MCP session binding is stale".into()),
        };
        let Some((step_id, revision)) = step else {
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(true);
        };
        if crate::role_routing::tool_ledger::find_by_operation(&transaction, &root_id, &operation_key)?.is_some() {
            transaction.commit().map_err(|error| error.to_string())?;
            return Ok(false);
        }
        let policy_json: String = transaction
            .query_row(
                "SELECT p.config_json FROM rr_roots r JOIN rr_policy_versions p ON p.id=r.policy_id WHERE r.root_id=?1",
                [&root_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        let policy: crate::role_routing::RoleRoutingSettings = serde_json::from_str(&policy_json)
            .map_err(|error| format!("Role-routing tool budget policy is invalid: {error}"))?;
        let used: i64 = transaction
            .query_row(
                "SELECT count(*) FROM rr_tool_links WHERE root_id=?1",
                [&root_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if used >= i64::from(policy.limits.max_tool_calls) {
            return Err("Role-routing tool budget exceeded".into());
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
) -> Result<(), String> {
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
    writer.write(move |connection| {
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
    })
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
    use rusqlite::Connection;

    fn role_writer() -> SqliteWriter {
        let connection = Connection::open_in_memory().expect("database");
        crate::initialize_database(&connection).expect("schema");
        connection.execute("INSERT INTO conversations(id,title,task_mode,created_at,updated_at) VALUES('c',NULL,'conversation','1','1')", []).expect("conversation");
        connection.execute("INSERT INTO runtime_runs(id,conversation_id,route_kind,status,started_at) VALUES('r','c','conversation.respond','running','1')", []).expect("run");
        let policy_id: String = connection
            .query_row(
                "SELECT id FROM rr_policy_versions ORDER BY version DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("policy");
        connection.execute("INSERT INTO rr_roots(root_id,conversation_id,runtime_run_id,policy_id,revision,phase,origin,presentation_mode,started_at_ms,scope_digest) VALUES('r','c','r',?1,0,'responding','text','visual',1,'')", [&policy_id]).expect("root");
        connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('s0','r',0,0,'actor','respond','running','f0','{}',10)", []).expect("step");
        SqliteWriter::from_connection(connection)
    }

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

    #[test]
    fn rr_21_old_session_cannot_use_new_step() {
        let writer = role_writer();
        let old = RoleStepBinding {
            root_id: "r",
            step_id: "s0",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        writer
            .write(|connection| {
                connection
                    .execute("UPDATE rr_steps SET status='succeeded',completed_at_ms=20 WHERE id='s0'", [])
                    .map_err(|error| error.to_string())?;
                connection
                    .execute("UPDATE rr_roots SET revision=1 WHERE root_id='r'", [])
                    .map_err(|error| error.to_string())?;
                connection.execute("INSERT INTO rr_steps(id,root_id,revision,ordinal,actor_id,purpose,status,config_fingerprint,adapter_state_json,started_at_ms) VALUES('s1','r',1,1,'actor','respond','running','f1','{}',20)", []).map_err(|error| error.to_string())?;
                Ok(())
            })
            .expect("advance root");
        assert!(reserve_routing_operation(&writer, "r", "old-operation", Some(&old)).is_err());
        let links = writer
            .read_serialized(|connection| {
                connection
                    .query_row("SELECT count(*) FROM rr_tool_links", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .map_err(|error| error.to_string())
            })
            .expect("links");
        assert_eq!(links, 0);
    }

    #[test]
    fn rr_21_missing_step_denied() {
        let writer = role_writer();
        let missing = RoleStepBinding {
            root_id: "r",
            step_id: "missing",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        assert!(reserve_routing_operation(&writer, "r", "missing-step", Some(&missing)).is_err());
        assert!(authorize_routing_tool(
            &writer,
            "r",
            crate::role_routing::tools::ToolEffect::ReadOnly,
            Some(&missing)
        )
        .is_err());
    }

    #[test]
    fn rr_21_tool_budget() {
        let writer = role_writer();
        writer
            .write(|connection| {
                let policy_id: String = connection
                    .query_row(
                        "SELECT policy_id FROM rr_roots WHERE root_id='r'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                let mut policy = crate::role_routing::RoleRoutingSettings::default();
                policy.limits.max_tool_calls = 0;
                connection
                    .execute(
                        "UPDATE rr_policy_versions SET config_json=?1 WHERE id=?2",
                        rusqlite::params![
                            serde_json::to_string(&policy).map_err(|error| error.to_string())?,
                            policy_id
                        ],
                    )
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .expect("zero tool budget");
        let binding = RoleStepBinding {
            root_id: "r",
            step_id: "s0",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        assert!(
            reserve_routing_operation(&writer, "r", "over-budget", Some(&binding))
                .expect_err("budget must reject")
                .contains("budget exceeded")
        );
        assert_eq!(
            writer
                .read_serialized(|connection| connection
                    .query_row("SELECT count(*) FROM rr_tool_links", [], |row| row
                        .get::<_, i64>(0))
                    .map_err(|error| error.to_string()))
                .expect("links"),
            0
        );
    }

    #[test]
    fn rr_10_update_between_reserve_and_invoke() {
        let writer = role_writer();
        writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE rr_roots SET cancel_requested=1,phase='draining' WHERE root_id='r'",
                        [],
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
            .expect("cancel root");
        let binding = RoleStepBinding {
            root_id: "r",
            step_id: "s0",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        assert!(reserve_routing_operation(&writer, "r", "cancelled", Some(&binding)).is_err());
    }

    #[test]
    fn rr_29_tool_permit_update_before_invoke() {
        let writer = role_writer();
        let binding = RoleStepBinding {
            root_id: "r",
            step_id: "s0",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        authorize_routing_tool(
            &writer,
            "r",
            crate::role_routing::tools::ToolEffect::ReadOnly,
            Some(&binding),
        )
        .expect("initial permit");
        writer
            .write(|connection| {
                connection
                    .execute(
                        "UPDATE rr_roots SET cancel_requested=1,phase='cancelled' WHERE root_id='r'",
                        [],
                    )
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
            .expect("condition update commits");
        assert!(
            reserve_routing_operation(&writer, "r", "must-not-reach-owner", Some(&binding))
                .is_err()
        );
        assert_eq!(
            writer
                .read_serialized(|connection| connection
                    .query_row("SELECT count(*) FROM rr_tool_links", [], |row| row
                        .get::<_, i64>(0))
                    .map_err(|error| error.to_string()))
                .expect("links"),
            0,
            "the second gateway check must stop invocation before owner reservation"
        );
    }

    #[test]
    fn rr_10_reviewer_resolved_mutation_denied_at_gateway() {
        let writer = role_writer();
        writer
            .write(|connection| {
                connection
                    .execute("UPDATE rr_steps SET purpose='review' WHERE id='s0'", [])
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
            .expect("review step");
        let binding = RoleStepBinding {
            root_id: "r",
            step_id: "s0",
            revision: 0,
            attempt_started_at_ms: 10,
            config_fingerprint: "f0",
        };
        assert!(authorize_routing_tool(
            &writer,
            "r",
            crate::role_routing::tools::ToolEffect::Mutating,
            Some(&binding),
        )
        .is_err());
        assert!(authorize_routing_tool(
            &writer,
            "r",
            crate::role_routing::tools::ToolEffect::ReadOnly,
            Some(&binding),
        )
        .is_ok());
    }

    #[test]
    fn rr_11_duplicate_operation_once_uses_canonical_payload() {
        assert_eq!(
            routing_operation_key("tools_invoke", r#"{"a":1,"b":{"x":2,"y":3}}"#),
            routing_operation_key("tools_invoke", r#"{"b":{"y":3,"x":2},"a":1}"#),
        );
        assert_ne!(
            routing_operation_key("tools_invoke", r#"{"a":1}"#),
            routing_operation_key("tools_invoke", r#"{"a":2}"#),
        );
    }

    #[test]
    fn rr_11_settle_failure_blocks_continuation() {
        let writer = role_writer();
        let error = settle_routing_operation(
            &writer,
            "r",
            "operation-that-was-never-reserved",
            &json!({"ok":true,"data":{"status":"succeeded"}}),
        )
        .expect_err("missing reservation must be visible to caller");
        assert!(error.contains("reservation is unavailable"));
    }
}
