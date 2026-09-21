//! Contract boundary for a tool-specialist child actor.
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
}
