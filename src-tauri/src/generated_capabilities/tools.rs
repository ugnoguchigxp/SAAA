//! Common execution adapter for generated capabilities.
//!
//! Conversation and MCP pass through here: name lookup happens against the immutable offer
//! snapshot, the host starts only through `CapabilityService::invoke`, and no path falls back.

use serde_json::{json, Map, Value};
use std::time::Duration;

use super::{
    contracts::{CallActor, InvocationResult, InvokeRequest},
    errors::*,
    guards::validate_input,
    host::process::Cancellation,
    limits,
    publication::{GeneratedToolSnapshot, GeneratedToolsConfig, MAX_INPUT_BYTES},
    service::CapabilityService,
};
use crate::runtime::agent_tools::AgentToolCall;
use crate::{AppState, RunCancellation};

/// Execution binding for one host-offered tool on this provider request. The reference is not
/// looked up again by tool name.
#[derive(Clone, Debug)]
pub(crate) struct DirectExecution {
    pub(crate) tool_name: String,
    pub(crate) execution_ref: String,
    pub(crate) conversation_id: String,
}

/// The tools offered to one provider request, together with the immutable generated-capability
/// snapshot they were built from, so the definitions and the revision they name cannot diverge.
pub(crate) struct AgentToolOffer {
    pub(crate) definitions: Vec<Value>,
    pub(crate) generated: GeneratedToolSnapshot,
    pub(crate) direct: Option<DirectExecution>,
}

impl AgentToolOffer {
    pub(crate) fn empty() -> Self {
        Self {
            definitions: Vec::new(),
            generated: GeneratedToolSnapshot::empty(),
            direct: None,
        }
    }
}

/// Resolves the generated snapshot for one request and appends its definitions. One catalog read;
/// any failure (disabled, DB, corrupt contract, duplicate name, size) publishes nothing.
pub(crate) fn append_generated(
    definitions: &mut Vec<Value>,
    state: &AppState,
) -> GeneratedToolSnapshot {
    let GeneratedToolsConfig {
        enabled,
        capability_ids,
        ..
    } = &state.generated_tools;
    if !enabled || !state.generated_capabilities.is_exposed() {
        return GeneratedToolSnapshot::empty();
    }
    let generated = match state
        .generated_capabilities
        .resolve_publication(capability_ids)
    {
        Ok(resolved) => GeneratedToolSnapshot::build(resolved).unwrap_or_default(),
        Err(_) => GeneratedToolSnapshot::empty(),
    };
    if generated.is_empty() {
        return generated;
    }
    // The neutral build already enforces the limit; re-check the exact provider array so adapter
    // wrappers cannot push it over. Over the limit means no generated tools at all, never a cut.
    let provider_definitions = openai_tool_definitions(&generated);
    if !provider_definitions_fit(&provider_definitions) {
        return GeneratedToolSnapshot::empty();
    }
    definitions.extend(provider_definitions);
    generated
}

/// OpenAI Chat Completions adapter over the provider-neutral snapshot descriptors.
pub(crate) fn openai_tool_definitions(snapshot: &GeneratedToolSnapshot) -> Vec<Value> {
    snapshot
        .definitions()
        .into_iter()
        .map(|function| json!({ "type": "function", "function": function }))
        .collect()
}

/// True when the provider-facing definitions, adapter wrappers included, fit the 32 KiB limit.
/// Kept separate from the snapshot so the wrapper accounting is directly testable.
pub(crate) fn provider_definitions_fit(provider_definitions: &[Value]) -> bool {
    serde_json::to_vec(provider_definitions)
        .map(|bytes| bytes.len() <= super::publication::MAX_DEFINITIONS_BYTES)
        .unwrap_or(false)
}

/// Runs one generated tool call and records the host-derived owner for later inspection.
pub(crate) async fn execute_with_actor(
    service: Option<&CapabilityService>,
    snapshot: &GeneratedToolSnapshot,
    call: &AgentToolCall,
    origin: &'static str,
    actor: Option<CallActor>,
    timeout: Duration,
    run_cancellation: &RunCancellation,
) -> String {
    let Some(service) = service else {
        return unavailable_content();
    };
    let Some(resolved) = snapshot.resolve(&call.name) else {
        return error_content(
            CapabilityErrorCode::InvalidInput,
            "The generated tool was not offered for this request.",
        );
    };
    if call.arguments.len() > MAX_INPUT_BYTES {
        return error_content(
            CapabilityErrorCode::InvalidInput,
            "The generated tool input is invalid.",
        );
    }
    let input: Map<String, Value> = match serde_json::from_str(&call.arguments) {
        Ok(input) => input,
        Err(_) => {
            return error_content(
                CapabilityErrorCode::InvalidInput,
                "The generated tool input is invalid.",
            )
        }
    };
    if validate_input(&resolved.contract, &input).is_err() {
        return error_content(
            CapabilityErrorCode::InvalidInput,
            "The generated tool input is invalid.",
        );
    }
    // An expired budget must be rejected before any durable call row exists.
    if timeout.as_millis() == 0 {
        return error_content(
            CapabilityErrorCode::Timeout,
            safe_message(CapabilityErrorCode::Timeout),
        );
    }

    // The provider call id is kept only for the conversation correlation; the durable call row
    // and the host request use a fresh host-generated UUID.
    let call_id = uuid::Uuid::new_v4().to_string();
    let mut request = InvokeRequest::new(resolved.clone(), call_id, input);
    request.origin = origin;
    request.actor = actor;
    request.inner_timeout_ms = timeout
        .as_millis()
        .clamp(1, limits::INNER_TIMEOUT_MAX_MS as u128) as u64;

    let host_cancel = Cancellation::default();
    // A run cancelled before the call starts must not start a host process at all; this is
    // checked before the invoke future is created so it is never polled.
    if run_cancellation.is_cancelled() {
        return error_content(
            CapabilityErrorCode::Cancelled,
            safe_message(CapabilityErrorCode::Cancelled),
        );
    }
    // If the provider attempt is dropped (outer timeout/abort), the detached host task still
    // owns the record and the permit; this guard makes sure it also receives the cancellation.
    let _cancel_on_drop = CancelOnDrop(host_cancel.clone());
    let deadline = timeout.min(limits::INVOKE_TIMEOUT);
    let mut invoke = Box::pin(service.invoke(request, &host_cancel));
    let interrupted = tokio::select! {
        biased;
        _ = run_cancellation.cancelled() => Some(CapabilityErrorCode::Cancelled),
        result = &mut invoke => return result_content(result),
        _ = tokio::time::sleep(deadline) => Some(CapabilityErrorCode::Timeout),
    };
    host_cancel.cancel();
    // The future is still owned here (the select only borrowed it), so wait for the detached
    // task to reach its terminal record and release the process slot.
    let _ = invoke.await;
    let code = interrupted.unwrap_or(CapabilityErrorCode::Cancelled);
    error_content(code, safe_message(code))
}

pub(crate) fn unavailable_content() -> String {
    error_content(
        CapabilityErrorCode::Unavailable,
        safe_message(CapabilityErrorCode::Unavailable),
    )
}

fn result_content(result: CapabilityResult<InvocationResult>) -> String {
    match result {
        Ok(invocation) => json!({
            "ok": true,
            "callId": invocation.call_id,
            "revisionId": invocation.revision_id,
            "value": invocation.value,
        })
        .to_string(),
        Err(error) => error_content(error.code, safe_message(error.code)),
    }
}

fn error_content(code: CapabilityErrorCode, message: &str) -> String {
    json!({ "ok": false, "error": { "code": code.as_str(), "message": message } }).to_string()
}

/// Fixed, non-leaking explanations. Internal messages, paths and stderr never reach the model.
fn safe_message(code: CapabilityErrorCode) -> &'static str {
    match code {
        CapabilityErrorCode::Disabled | CapabilityErrorCode::Unavailable => {
            "The generated tool is unavailable."
        }
        CapabilityErrorCode::UnsupportedContract
        | CapabilityErrorCode::UnsupportedPackageLayout
        | CapabilityErrorCode::InvalidPackage => "The generated tool package is not supported.",
        CapabilityErrorCode::InvalidInput => "The generated tool input is invalid.",
        CapabilityErrorCode::IntegrityError => "The generated tool failed its integrity check.",
        CapabilityErrorCode::VerificationFailed | CapabilityErrorCode::NotValidated => {
            "The generated tool is not verified for the current runtime."
        }
        CapabilityErrorCode::NotActive | CapabilityErrorCode::StaleRevision => {
            "The generated tool is no longer active."
        }
        CapabilityErrorCode::Conflict => "The generated tool changed while it was called.",
        CapabilityErrorCode::Busy => "The generated tool runtime is busy. Try again.",
        CapabilityErrorCode::Timeout => "The generated tool timed out.",
        CapabilityErrorCode::Cancelled => "The generated tool call was cancelled.",
        CapabilityErrorCode::OutputLimit => "The generated tool produced too much output.",
        CapabilityErrorCode::ProtocolError => {
            "The generated tool runtime returned an invalid response."
        }
        CapabilityErrorCode::StorageError => "The generated tool result could not be recorded.",
    }
}

struct CancelOnDrop(Cancellation);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
