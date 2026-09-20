//! Host-side execution budget checks. These functions have no clock or provider dependency.
use super::contracts::RoutingLimits;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LimitViolation {
    RootTimeout,
    StepTimeout,
    CostUnknown,
    CostExceeded,
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
        let mut limits = RoutingLimits::default();
        limits.max_estimated_cost_micros = Some(10);
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
    fn rr_22_deadline_rejects_new_dispatch() {
        let mut limits = RoutingLimits::default();
        limits.root_timeout_ms = 10;
        limits.step_timeout_ms = 5;
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
