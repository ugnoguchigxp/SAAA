use super::*;
#[cfg(any(test, feature = "offline-contracts"))]
use crate::runtime::agent_tools;
use crate::{ProviderOutputPersistence, StartTurnInput};
use serde_json::Value;

pub(super) async fn execute_artifact_webview(
    output_persistence: Option<ProviderOutputPersistence<'_>>,
    input: &StartTurnInput,
    call: &crate::runtime::agent_tools::AgentToolCall,
    run_cancellation: &RunCancellation,
    direct: Option<&crate::generated_capabilities::tools::DirectExecution>,
) -> String {
    let Some(persistence) = output_persistence else {
        return serde_json::json!({"ok": false, "reason": "not-offered", "stage": "offer"})
            .to_string();
    };
    let Some(direct) = direct.filter(|direct| {
        direct.tool_name == call.name && direct.conversation_id == input.conversation_id
    }) else {
        return serde_json::json!({"ok": false, "reason": "not-offered", "stage": "offer"})
            .to_string();
    };
    let execution_ref = &direct.execution_ref;
    let arguments: Value = match serde_json::from_str(&call.arguments) {
        Ok(value) => value,
        Err(_) => {
            return serde_json::json!({"ok": false, "reason": "invalid-input", "stage": "schema"})
                .to_string()
        }
    };
    let Ok(principal) =
        crate::tool_selection::service::ensure_principal(&persistence.state.sqlite_writer)
    else {
        return serde_json::json!({"ok": false, "reason": "not-authorized", "stage": "grant"})
            .to_string();
    };
    let context = crate::tool_selection::RequestContext::new(&principal, &input.conversation_id)
        .with_run(Some(input.run_id.clone()));
    match persistence
        .state
        .tool_selection
        .invoke(&context, execution_ref, &arguments, run_cancellation)
        .await
    {
        Ok(response) => serde_json::json!({
            "ok": response.status == crate::tool_selection::backends::TechnicalStatus::Succeeded,
            "stage": "invoke",
            "invocationId": response.invocation_id,
            "runId": input.run_id,
            "status": response.status.as_str(),
            "errorCode": response.error_code,
            "result": response.result,
        })
        .to_string(),
        Err(error) => serde_json::json!({
            "ok": false,
            "stage": "invoke",
            "runId": input.run_id,
            "reason": error.code.as_str(),
            "message": error.message,
        })
        .to_string(),
    }
}
