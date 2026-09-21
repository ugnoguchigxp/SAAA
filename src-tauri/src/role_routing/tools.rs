//! Host-enforced role tool permits. Prompts are never the authorization boundary.
use std::collections::HashSet;

/// Trusted side-effect classification of the resolved tool. It comes from the tool catalog's
/// `effect` column, never from the tool name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolEffect {
    ReadOnly,
    Mutating,
}

/// Maps the catalog effect to the permit classification. `pure`/`read` are read-only; everything
/// else, including `unknown` and a missing catalog entry, is treated as mutating (fail closed).
pub(crate) fn classify_effect(effect: Option<&str>) -> ToolEffect {
    match effect {
        Some("pure" | "read") => ToolEffect::ReadOnly,
        _ => ToolEffect::Mutating,
    }
}

pub(crate) fn permits(
    role: &str,
    tool_name: &str,
    offered: &[String],
    revision_matches: bool,
    effect: ToolEffect,
) -> Result<(), &'static str> {
    if !revision_matches {
        return Err("stale_revision");
    }
    if !offered.iter().any(|tool| tool == tool_name) {
        return Err("unoffered_tool");
    }
    permits_effect(role, effect)
}

/// Role check independent of the exact tool name or offered set. Used by the gateway for every
/// routing tool call so a reviewer can never reach a mutating tool even if it was offered.
pub(crate) fn permits_effect(role: &str, effect: ToolEffect) -> Result<(), &'static str> {
    let allowed = match role {
        "frontend" => false,
        "reviewer" => effect == ToolEffect::ReadOnly,
        "reasoner" | "advanced" | "premium" | "tool_specialist" => true,
        _ => false,
    };
    allowed.then_some(()).ok_or("role_tool_denied")
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
            permits(
                "frontend",
                "search",
                &["search".into()],
                true,
                ToolEffect::ReadOnly
            ),
            Err("role_tool_denied")
        );
    }
    #[test]
    fn rr_10_unoffered_tool() {
        assert_eq!(
            permits(
                "reasoner",
                "delete",
                &["search".into()],
                true,
                ToolEffect::Mutating
            ),
            Err("unoffered_tool")
        );
    }
    #[test]
    fn rr_10_revision_before_permit() {
        assert_eq!(
            permits(
                "reasoner",
                "search",
                &["search".into()],
                false,
                ToolEffect::ReadOnly
            ),
            Err("stale_revision")
        );
    }

    #[test]
    fn rr_24_reviewer_cannot_call_a_mutating_tool() {
        // The decision uses the trusted effect, not the tool name: a tool named `search` that is
        // actually mutating is denied for the reviewer.
        assert_eq!(
            permits(
                "reviewer",
                "search",
                &["search".into()],
                true,
                ToolEffect::Mutating
            ),
            Err("role_tool_denied")
        );
        assert!(permits(
            "reviewer",
            "search",
            &["search".into()],
            true,
            ToolEffect::ReadOnly
        )
        .is_ok());
    }

    #[test]
    fn rr_10_unknown_effect_is_not_read_only() {
        assert_eq!(classify_effect(None), ToolEffect::Mutating);
        assert_eq!(classify_effect(Some("unknown")), ToolEffect::Mutating);
        assert_eq!(classify_effect(Some("read")), ToolEffect::ReadOnly);
        assert_eq!(classify_effect(Some("pure")), ToolEffect::ReadOnly);
    }
}
