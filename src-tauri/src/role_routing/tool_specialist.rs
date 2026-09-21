//! Contract boundary for a tool-specialist child actor.
use crate::persistence::SqliteWriter;
use crate::tool_selection::ToolSelectionService;
use crate::RunCancellation;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SpecialistRequest {
    #[serde(rename = "toolName")]
    pub(crate) tool_name: String,
    pub(crate) arguments: serde_json::Value,
}

/// The specialist never gains final-answer authority; the parent interprets every tool result.
pub(crate) fn validate(request: &SpecialistRequest, enabled: bool) -> Result<(), String> {
    if !enabled || request.tool_name.is_empty() || !request.arguments.is_object() {
        return Err("Role-routing tool specialist request is unavailable".into());
    }
    Ok(())
}

/// Executes a specialist request through the same host-owned gateway used by the restricted
/// role MCP bridge. The specialist returns the gateway envelope to its parent; it has no route
/// to publish a conversation answer or bypass the role-root tool ledger.
pub(crate) async fn execute_for_root(
    service: &ToolSelectionService,
    writer: &SqliteWriter,
    conversation_id: &str,
    root_id: &str,
    input_message_id: Option<String>,
    request: &SpecialistRequest,
    enabled: bool,
    offered_tools: &[String],
    revision_matches: bool,
    cancellation: &RunCancellation,
) -> Result<serde_json::Value, String> {
    validate(request, enabled)?;
    let effect = writer.read_serialized(|connection| {
        crate::tool_selection::repository::effect_for_backend_key(connection, &request.tool_name)
    })?;
    super::tools::permits(
        "tool_specialist",
        &request.tool_name,
        offered_tools,
        revision_matches,
        super::tools::classify_effect(effect.as_deref()),
    )
    .map_err(str::to_string)?;
    let arguments = serde_json::to_string(&request.arguments)
        .map_err(|_| "Role-routing tool specialist arguments are invalid".to_string())?;
    Ok(crate::tool_selection::gateway::execute_for_role_root(
        service,
        writer,
        conversation_id,
        root_id,
        input_message_id,
        &request.tool_name,
        &arguments,
        cancellation,
    )
    .await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_38_specialist_disabled_is_rejected() {
        let request = SpecialistRequest {
            tool_name: "search".into(),
            arguments: serde_json::json!({"q":"x"}),
        };
        assert!(validate(&request, false).is_err());
    }

    #[test]
    fn rr_38_specialist_requires_a_host_tool_request() {
        let request = SpecialistRequest {
            tool_name: "".into(),
            arguments: serde_json::json!({}),
        };
        assert!(validate(&request, true).is_err());
    }

    #[test]
    fn rr_38_specialist_uses_the_same_host_tool_permits() {
        let request = SpecialistRequest {
            tool_name: "tools_search".into(),
            arguments: serde_json::json!({"query":"x"}),
        };
        assert!(validate(&request, true).is_ok());
        assert!(super::super::tools::permits(
            "tool_specialist",
            &request.tool_name,
            &[request.tool_name.clone()],
            true,
            super::super::tools::ToolEffect::Mutating,
        )
        .is_ok());
        assert!(super::super::tools::permits(
            "tool_specialist",
            &request.tool_name,
            &[request.tool_name.clone()],
            false,
            super::super::tools::ToolEffect::ReadOnly,
        )
        .is_err());
    }
}
