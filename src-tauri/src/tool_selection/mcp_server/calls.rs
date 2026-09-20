//! Managed tool calls for the MCP server.
//!
//! The HTTP handler never owns the backend execution: it reserves a typed request id, hands the
//! call to a spawned task and awaits the reply with a deadline. Dropping the handler (client
//! disconnect) is not a cancellation; the call keeps running and the ledger still reaches a
//! terminal state. Only an explicit cancellation notification, DELETE, the deadline or shutdown
//! stops the call.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::protocol::{JsonRpcError, TypedRequestId, INVALID_PARAMS};
use super::sessions::Session;
use super::ServerInner;
use crate::tool_selection::gateway;
use crate::tool_selection::RequestContext;
use crate::RunCancellation;

/// Whole-call deadline. The lower backend timeouts are never extended by this budget.
pub const CALL_DEADLINE: Duration = Duration::from_secs(30);

/// The three fixed tool definitions, derived from the shared gateway schemas (never duplicated).
pub fn tools_list() -> Value {
    let definitions = gateway::definitions();
    let tools: Vec<Value> = definitions
        .into_iter()
        .filter_map(|definition| {
            let function = definition.get("function")?;
            let name = function.get("name")?.as_str()?.to_string();
            let mut description = function
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let guidance = match name.as_str() {
                "tools_search" => "Call this first; a candidate is not an execution decision.",
                "tools_describe" => {
                    "Required before tools_invoke: it returns the executionRef for one tool."
                }
                "tools_invoke" => "If the result is unknown, do not retry automatically.",
                _ => "",
            };
            if !guidance.is_empty() {
                description = format!("{description} {guidance}");
            }
            Some(json!({
                "name": name,
                "description": description,
                "inputSchema": function.get("parameters")?,
            }))
        })
        .collect();
    json!({ "tools": tools })
}

/// Executes one `tools/call` body and returns the MCP `result` object. Unknown tool names and
/// malformed outer params are invalid params; business failures are reported inside the result
/// with `isError: true`.
pub async fn execute_tool_call(
    service: &crate::tool_selection::ToolSelectionService,
    context: &RequestContext,
    name: &str,
    arguments: &Value,
    cancellation: &RunCancellation,
) -> Result<Value, JsonRpcError> {
    if gateway::internal_name(name).is_none() {
        return Err(JsonRpcError::new(INVALID_PARAMS, "Unknown tool"));
    }
    if !arguments.is_object() {
        return Err(JsonRpcError::new(INVALID_PARAMS, "Invalid params"));
    }
    let argument_text = serde_json::to_string(arguments)
        .map_err(|_| JsonRpcError::new(INVALID_PARAMS, "Invalid params"))?;
    let envelope =
        gateway::dispatch_external(service, context, name, &argument_text, cancellation).await;
    let is_error = envelope_is_error(&envelope);
    let text = serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".to_string());
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error
    }))
}

/// A gateway `ok:false`, or a terminal invoke status other than `succeeded`, is an error. A
/// succeeded call whose large result could not be stored stays a non-error so the client can see
/// `resultAvailability`.
fn envelope_is_error(envelope: &Value) -> bool {
    if envelope.get("ok").and_then(Value::as_bool) != Some(true) {
        return true;
    }
    matches!(
        envelope.pointer("/data/status").and_then(Value::as_str),
        Some("failed" | "cancelled" | "unknown" | "interrupted")
    )
}

/// Releases the reserved call slot exactly once, including when the HTTP handler future is dropped
/// while the call is still running.
pub struct CallPermit {
    inner: Arc<ServerInner>,
    session: Arc<Session>,
    id: TypedRequestId,
}

impl CallPermit {
    pub fn new(inner: Arc<ServerInner>, session: Arc<Session>, id: TypedRequestId) -> Self {
        Self { inner, session, id }
    }
}

impl Drop for CallPermit {
    fn drop(&mut self) {
        self.inner.sessions.finish_call(&self.session, &self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_list_has_exactly_three_shared_schemas() {
        let value = tools_list();
        let tools = value.get("tools").and_then(Value::as_array).expect("tools");
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            vec!["tools_search", "tools_describe", "tools_invoke"]
        );
        // The schema is the shared gateway schema, not a copy.
        assert_eq!(tools[0].get("inputSchema"), Some(&gateway::search_schema()));
        assert!(
            serde_json::to_vec(&value).expect("serialize").len() < 32 * 1024,
            "the three definitions must fit the 32 KiB bound"
        );
    }

    #[test]
    fn is_error_tracks_terminal_statuses_but_not_storage_limits() {
        assert!(envelope_is_error(&json!({"ok": false})));
        assert!(envelope_is_error(
            &json!({"ok": true, "data": {"status": "unknown"}})
        ));
        assert!(!envelope_is_error(
            &json!({"ok": true, "data": {"status": "succeeded", "resultAvailability": "unavailable"}})
        ));
        // A boolean `false` result is a valid success, not an error.
        assert!(!envelope_is_error(
            &json!({"ok": true, "data": {"status": "succeeded", "result": false}})
        ));
    }
}
