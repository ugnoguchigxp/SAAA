//! Host-enforced role tool permits. Prompts are never the authorization boundary.
use std::collections::HashSet;

pub(crate) fn permits(
    role: &str,
    tool_name: &str,
    offered: &[String],
    revision_matches: bool,
) -> Result<(), &'static str> {
    if !revision_matches {
        return Err("stale_revision");
    }
    if !offered.iter().any(|tool| tool == tool_name) {
        return Err("unoffered_tool");
    }
    let allowed = match role {
        "frontend" => false,
        "reviewer" => is_read_only(tool_name),
        "reasoner" | "advanced" | "premium" | "tool_specialist" => true,
        _ => false,
    };
    allowed.then_some(()).ok_or("role_tool_denied")
}

fn is_read_only(tool: &str) -> bool {
    ["search", "describe", "read", "list"]
        .iter()
        .any(|prefix| tool.starts_with(prefix))
}

pub(crate) fn unique_operation_keys(keys: &[String]) -> bool {
    keys.iter().collect::<HashSet<_>>().len() == keys.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rr_10_frontend_tool_rejected() {
        assert_eq!(
            permits("frontend", "search", &["search".into()], true),
            Err("role_tool_denied")
        );
    }
    #[test]
    fn rr_10_unoffered_tool() {
        assert_eq!(
            permits("reasoner", "delete", &["search".into()], true),
            Err("unoffered_tool")
        );
    }
    #[test]
    fn rr_10_revision_before_permit() {
        assert_eq!(
            permits("reasoner", "search", &["search".into()], false),
            Err("stale_revision")
        );
    }
}
