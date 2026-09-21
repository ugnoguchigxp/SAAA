//! Host-side validation for independent review results.
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewIssue {
    pub(crate) code: String,
    #[serde(rename = "evidenceRef")]
    pub(crate) evidence_ref: String,
    pub(crate) verdict: String,
}

pub(crate) fn validate(
    author_actor: &str,
    reviewer_actor: &str,
    issues: &[ReviewIssue],
    allowed_evidence_refs: &[String],
) -> Result<(), String> {
    if author_actor.is_empty() || reviewer_actor.is_empty() || author_actor == reviewer_actor {
        return Err("Role-routing review actor is not independent".into());
    }
    for issue in issues {
        if issue.code.is_empty()
            || !matches!(
                issue.verdict.as_str(),
                "verified" | "unverified" | "unresolved"
            )
            || !allowed_evidence_refs
                .iter()
                .any(|reference| reference == &issue.evidence_ref)
        {
            return Err("Role-routing review issue is invalid".into());
        }
    }
    Ok(())
}

/// A review may cause at most one bounded rework cycle per policy allowance. Unsupported or
/// unresolved findings never authorize an automatic revision.
pub(crate) fn revision_allowed(
    verified_issue_count: usize,
    completed_rounds: u8,
    max_rounds: u8,
) -> bool {
    verified_issue_count > 0 && completed_rounds < max_rounds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_24_self_review_not_independent() {
        assert!(validate("sol", "sol", &[], &[]).is_err());
    }

    #[test]
    fn rr_24_false_evidence_is_rejected() {
        let issue = ReviewIssue {
            code: "missing-citation".into(),
            evidence_ref: "unknown".into(),
            verdict: "verified".into(),
        };
        assert!(validate("sol", "qwen", &[issue], &["answer-1".into()]).is_err());
    }

    #[test]
    fn rr_25_review_round_limit_requires_verified_issue() {
        assert!(revision_allowed(1, 0, 1));
        assert!(!revision_allowed(0, 0, 1));
        assert!(!revision_allowed(1, 1, 1));
    }
}
