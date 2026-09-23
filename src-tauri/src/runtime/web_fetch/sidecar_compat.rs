//! Adapt the Rust tool schema to the rollback sidecar schema.
use super::{
    contracts::{FetchContentInput, WebFetchCancel},
    sidecar, AgentToolCall, FETCH_CONTENT_TOOL_NAME,
};
use crate::runtime::agent_tools::tool_error_content;
use serde_json::Value;
use std::time::Duration;

pub(super) async fn execute_via_sidecar(
    call: &AgentToolCall,
    timeout: Duration,
    cancellation: WebFetchCancel,
) -> String {
    let compatible_call = match sidecar_compatible_call(call) {
        Ok(call) => call,
        Err(()) => {
            return tool_error_content(
                "INVALID_INPUT",
                "Tool arguments do not match the WebFetch schema.",
            );
        }
    };
    let request = match sidecar::envelope_for_call(&compatible_call) {
        Ok(request) => request,
        Err(message) => {
            if message.contains("schema") {
                return tool_error_content("INVALID_INPUT", &message);
            }
            return tool_error_content("web-fetch-unavailable", &message);
        }
    };
    sidecar::execute_envelope(&request, timeout, cancellation).await
}

pub(super) fn sidecar_compatible_call(call: &AgentToolCall) -> Result<AgentToolCall, ()> {
    let mut compatible_call = call.clone();
    if call.name == FETCH_CONTENT_TOOL_NAME {
        let mut arguments = serde_json::from_str::<Value>(&call.arguments).map_err(|_| ())?;
        FetchContentInput::parse(&arguments).map_err(|_| ())?;
        if let Some(object) = arguments.as_object_mut() {
            object.remove("query");
            if object.get("maxCharacters").is_none_or(Value::is_null) {
                object.insert("maxCharacters".to_string(), Value::from(2_500));
            }
            compatible_call.arguments = arguments.to_string();
        }
    }
    Ok(compatible_call)
}
