//! Explicit approval gate for premium reasoning proposals.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Proposal {
    pub(crate) candidate_id: String,
    pub(crate) policy_id: String,
    pub(crate) revision: u32,
    pub(crate) expires_at_ms: i64,
}

pub(crate) fn may_dispatch(
    proposal: &Proposal,
    approved_candidate: Option<&str>,
    policy_id: &str,
    revision: u32,
    now_ms: i64,
    cloud_allowed: bool,
) -> bool {
    cloud_allowed
        && now_ms <= proposal.expires_at_ms
        && proposal.policy_id == policy_id
        && proposal.revision == revision
        && approved_candidate == Some(proposal.candidate_id.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal() -> Proposal {
        Proposal {
            candidate_id: "astra".into(),
            policy_id: "policy-1".into(),
            revision: 2,
            expires_at_ms: 100,
        }
    }

    #[test]
    fn rr_26_premium_no_implicit_execution() {
        assert!(!may_dispatch(&proposal(), None, "policy-1", 2, 10, true));
        assert!(may_dispatch(
            &proposal(),
            Some("astra"),
            "policy-1",
            2,
            10,
            true
        ));
    }

    #[test]
    fn rr_26_stale_or_cloud_forbidden_is_rejected() {
        assert!(!may_dispatch(
            &proposal(),
            Some("astra"),
            "policy-1",
            3,
            10,
            true
        ));
        assert!(!may_dispatch(
            &proposal(),
            Some("astra"),
            "policy-1",
            2,
            101,
            true
        ));
        assert!(!may_dispatch(
            &proposal(),
            Some("astra"),
            "policy-1",
            2,
            10,
            false
        ));
    }
}
