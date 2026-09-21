//! Host-side execution budget checks. These functions have no clock or provider dependency.
#![allow(dead_code)]
use super::contracts::RoutingLimits;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LimitViolation {
    RootTimeout,
    StepTimeout,
    CostUnknown,
    CostExceeded,
    StepBudget,
    ToolBudget,
    SwitchBudget,
    ReviewBudget,
}

/// Cumulative root budget. Limits apply to every execution of the root and are never reset by a
/// failure or an inputRevision update.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BudgetState {
    pub(crate) steps: u8,
    pub(crate) tool_calls: u8,
    pub(crate) automatic_switches: u8,
    pub(crate) review_rounds: u8,
    pub(crate) spent_cost_micros: u64,
}

/// Reserves one reasoning step. The state is returned only on success; a failure leaves the
/// caller's state untouched so no partial reservation is leaked.
pub(crate) fn reserve_step(
    limits: &RoutingLimits,
    state: BudgetState,
) -> Result<BudgetState, LimitViolation> {
    if state.steps >= limits.max_reasoning_steps {
        return Err(LimitViolation::StepBudget);
    }
    Ok(BudgetState {
        steps: state.steps + 1,
        ..state
    })
}

pub(crate) fn reserve_tool_calls(
    limits: &RoutingLimits,
    state: BudgetState,
    count: u8,
) -> Result<BudgetState, LimitViolation> {
    if state.tool_calls.saturating_add(count) > limits.max_tool_calls {
        return Err(LimitViolation::ToolBudget);
    }
    Ok(BudgetState {
        tool_calls: state.tool_calls.saturating_add(count),
        ..state
    })
}

pub(crate) fn reserve_automatic_switch(
    limits: &RoutingLimits,
    state: BudgetState,
) -> Result<BudgetState, LimitViolation> {
    if state.automatic_switches >= limits.max_automatic_switches {
        return Err(LimitViolation::SwitchBudget);
    }
    Ok(BudgetState {
        automatic_switches: state.automatic_switches + 1,
        ..state
    })
}

pub(crate) fn reserve_review_round(
    limits: &RoutingLimits,
    state: BudgetState,
) -> Result<BudgetState, LimitViolation> {
    if state.review_rounds >= limits.max_review_rounds {
        return Err(LimitViolation::ReviewBudget);
    }
    Ok(BudgetState {
        review_rounds: state.review_rounds + 1,
        ..state
    })
}

/// Two dispatches may run at the same time only when they use different resource groups. The same
/// group is serialized, and the reasoner wins the slot over frontend/classify work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResourceRequest<'a> {
    pub(crate) actor_id: &'a str,
    pub(crate) role: &'a str,
    pub(crate) resource_group: &'a str,
}

pub(crate) fn can_run_concurrently(
    left: &ResourceRequest<'_>,
    right: &ResourceRequest<'_>,
) -> bool {
    left.resource_group != right.resource_group
}

fn role_priority(role: &str) -> u8 {
    match role {
        "reasoner" | "advanced" | "premium" => 0,
        _ => 1,
    }
}

/// Orders requests so that within a shared resource group the reasoner is dispatched first.
pub(crate) fn order_by_resource_group_priority(requests: &mut [ResourceRequest<'_>]) {
    requests.sort_by(|left, right| {
        left.resource_group
            .cmp(right.resource_group)
            .then_with(|| role_priority(left.role).cmp(&role_priority(right.role)))
            .then_with(|| left.actor_id.cmp(right.actor_id))
    });
}

/// Checks whether a step may be dispatched at `now_ms`. `next_cost_micros` is an estimate supplied
/// by the host adapter; when policy has a cost limit, absence is deliberately not treated as zero.
pub(crate) fn permit_dispatch(
    limits: &RoutingLimits,
    root_started_at_ms: i64,
    step_started_at_ms: Option<i64>,
    now_ms: i64,
    spent_cost_micros: u64,
    next_cost_micros: Option<u64>,
) -> Result<(), LimitViolation> {
    if now_ms.saturating_sub(root_started_at_ms) > limits.root_timeout_ms as i64 {
        return Err(LimitViolation::RootTimeout);
    }
    if step_started_at_ms
        .is_some_and(|started| now_ms.saturating_sub(started) > limits.step_timeout_ms as i64)
    {
        return Err(LimitViolation::StepTimeout);
    }
    let Some(max_cost) = limits.max_estimated_cost_micros else {
        return Ok(());
    };
    let Some(next_cost) = next_cost_micros else {
        return Err(LimitViolation::CostUnknown);
    };
    if spent_cost_micros
        .checked_add(next_cost)
        .is_none_or(|total| total > max_cost)
    {
        return Err(LimitViolation::CostExceeded);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_22_unknown_cost_is_not_free_when_policy_has_a_budget() {
        let limits = RoutingLimits {
            max_estimated_cost_micros: Some(10),
            ..Default::default()
        };
        assert_eq!(
            permit_dispatch(&limits, 0, None, 1, 0, None),
            Err(LimitViolation::CostUnknown)
        );
        assert_eq!(
            permit_dispatch(&limits, 0, None, 1, 8, Some(3)),
            Err(LimitViolation::CostExceeded)
        );
    }

    #[test]
    fn rr_22_loop_budget() {
        let limits = RoutingLimits::default();
        let mut state = BudgetState::default();
        for _ in 0..limits.max_reasoning_steps {
            state = reserve_step(&limits, state).expect("step within budget");
        }
        assert_eq!(
            reserve_step(&limits, state),
            Err(LimitViolation::StepBudget)
        );
        let mut tool_state = BudgetState::default();
        tool_state = reserve_tool_calls(&limits, tool_state, 31).expect("tools");
        assert_eq!(
            reserve_tool_calls(&limits, tool_state, 2),
            Err(LimitViolation::ToolBudget)
        );
        let mut switch_state = BudgetState::default();
        for _ in 0..limits.max_automatic_switches {
            switch_state = reserve_automatic_switch(&limits, switch_state).expect("switch");
        }
        assert_eq!(
            reserve_automatic_switch(&limits, switch_state),
            Err(LimitViolation::SwitchBudget)
        );
    }

    #[test]
    fn rr_09_shared_resource_group() {
        let reasoner = ResourceRequest {
            actor_id: "qwen",
            role: "reasoner",
            resource_group: "gpu",
        };
        let frontend = ResourceRequest {
            actor_id: "front",
            role: "frontend",
            resource_group: "gpu",
        };
        let other = ResourceRequest {
            actor_id: "sol",
            role: "advanced",
            resource_group: "cloud",
        };
        assert!(!can_run_concurrently(&reasoner, &frontend));
        assert!(can_run_concurrently(&reasoner, &other));
        let mut requests = vec![frontend, reasoner, other];
        order_by_resource_group_priority(&mut requests);
        assert_eq!(requests[0].actor_id, "sol");
        assert_eq!(requests[1].actor_id, "qwen");
        assert_eq!(requests[2].actor_id, "front");
    }

    #[test]
    fn rr_22_deadline_rejects_new_dispatch() {
        let limits = RoutingLimits {
            root_timeout_ms: 10,
            step_timeout_ms: 5,
            ..Default::default()
        };
        assert_eq!(
            permit_dispatch(&limits, 0, None, 11, 0, Some(0)),
            Err(LimitViolation::RootTimeout)
        );
        assert_eq!(
            permit_dispatch(&limits, 0, Some(1), 7, 0, Some(0)),
            Err(LimitViolation::StepTimeout)
        );
    }
}
