#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RebuildReason {
    Budget,
    ScopeChange,
    PolicyVersion,
    Forget,
}

pub(crate) struct RebuildInput<'a> {
    pub(crate) estimated_bytes: usize,
    pub(crate) budget: usize,
    pub(crate) omissible_entries: bool,
    pub(crate) scope_changed: bool,
    pub(crate) policy_changed: bool,
    pub(crate) tool_digest_changed: bool,
    pub(crate) forget_changed: bool,
    pub(crate) previous_start_reason: &'a str,
}

pub(crate) fn needs_rebuild(input: &RebuildInput<'_>) -> Option<RebuildReason> {
    if input.scope_changed {
        return Some(RebuildReason::ScopeChange);
    }
    if input.policy_changed || input.tool_digest_changed {
        return Some(RebuildReason::PolicyVersion);
    }
    if input.forget_changed {
        return Some(RebuildReason::Forget);
    }
    if input.estimated_bytes > input.budget * 70 / 100 && input.omissible_entries {
        if input.previous_start_reason == "budget" {
            return None;
        }
        return Some(RebuildReason::Budget);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(estimated: usize, budget: usize, reason: &str) -> RebuildInput<'_> {
        RebuildInput {
            estimated_bytes: estimated,
            budget,
            omissible_entries: true,
            scope_changed: false,
            policy_changed: false,
            tool_digest_changed: false,
            forget_changed: false,
            previous_start_reason: reason,
        }
    }

    #[test]
    fn cw_44_budget_rebuild_at_70_percent() {
        assert_eq!(
            needs_rebuild(&input(71, 100, "initial")),
            Some(RebuildReason::Budget)
        );
        assert_eq!(needs_rebuild(&input(70, 100, "initial")), None);
    }

    #[test]
    fn cw_44_no_consecutive_budget_rebuild() {
        assert_eq!(needs_rebuild(&input(90, 100, "budget")), None);
    }
}
