//! Host-side validation for independent review results.
//!
//! Review prompts deliberately carry answer material and evidence references, but never an
//! author model/actor identifier.  Independence is established from the durable step ledger,
//! not from a model-supplied claim in the review payload.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReviewRequest {
    pub(crate) answer: String,
    pub(crate) evidence_refs: Vec<String>,
}

impl ReviewRequest {
    pub(crate) fn new(answer: String, evidence_refs: Vec<String>) -> Result<Self, String> {
        if answer.trim().is_empty()
            || evidence_refs.is_empty()
            || evidence_refs.iter().any(|reference| reference.is_empty())
        {
            return Err("Role-routing review request is incomplete".into());
        }
        if evidence_refs.len()
            != evidence_refs
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
        {
            return Err("Role-routing review request repeats evidence".into());
        }
        Ok(Self {
            answer,
            evidence_refs,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewIssue {
    /// condition / logic / evidence / calculation / tool_result
    pub(crate) kind: String,
    pub(crate) claim: String,
    /// minor / major
    pub(crate) severity: String,
    pub(crate) code: String,
    #[serde(rename = "evidenceRef")]
    pub(crate) evidence_ref: String,
    /// Model-supplied advisory verdict. It never authorizes a revision on its own.
    pub(crate) verdict: String,
}

/// Host-derived verification, kept separate from the model's advisory verdict so a model cannot
/// authorize a revision by writing the string `verified`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HostVerification {
    Verified,
    Unverified,
}

const MAX_ISSUES: usize = 8;
const MAX_CLAIM_BYTES: usize = 2_000;

fn valid_kind(kind: &str) -> bool {
    matches!(
        kind,
        "condition" | "logic" | "evidence" | "calculation" | "tool_result"
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewResponse {
    pub(crate) issues: Vec<ReviewIssue>,
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
    if issues.len() > MAX_ISSUES {
        return Err("Role-routing review has too many issues".into());
    }
    for issue in issues {
        if issue.code.is_empty()
            || issue.claim.trim().is_empty()
            || issue.claim.len() > MAX_CLAIM_BYTES
            || !valid_kind(&issue.kind)
            || !matches!(issue.severity.as_str(), "minor" | "major")
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

/// Verifies an issue from host-owned facts. A model `verified` verdict only becomes host-verified
/// when the evidence reference is in scope and has not been revoked; anything else stays
/// unverified. The raw model verdict is preserved for audit but is not an authorization.
pub(crate) fn host_verify(
    issue: &ReviewIssue,
    allowed_evidence_refs: &[String],
    revoked_evidence_refs: &[String],
) -> HostVerification {
    let in_scope = allowed_evidence_refs
        .iter()
        .any(|reference| reference == &issue.evidence_ref);
    let revoked = revoked_evidence_refs
        .iter()
        .any(|reference| reference == &issue.evidence_ref);
    if issue.verdict == "verified" && in_scope && !revoked && !issue.claim.trim().is_empty() {
        HostVerification::Verified
    } else {
        HostVerification::Unverified
    }
}

/// Counts issues that passed host verification. This is the only count that may authorize a
/// revision.
pub(crate) fn host_verified_count(
    issues: &[ReviewIssue],
    allowed_evidence_refs: &[String],
    revoked_evidence_refs: &[String],
) -> usize {
    issues
        .iter()
        .filter(|issue| {
            host_verify(issue, allowed_evidence_refs, revoked_evidence_refs)
                == HostVerification::Verified
        })
        .count()
}

/// A review may cause at most one bounded rework cycle per policy allowance. Unsupported or
/// unresolved findings never authorize an automatic revision.
pub(crate) fn revision_allowed(
    host_verified_issue_count: usize,
    completed_rounds: u8,
    max_rounds: u8,
) -> bool {
    host_verified_issue_count > 0 && completed_rounds < max_rounds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(evidence_ref: &str, verdict: &str) -> ReviewIssue {
        ReviewIssue {
            kind: "evidence".into(),
            claim: "the cited condition is not met".into(),
            severity: "major".into(),
            code: "missing-citation".into(),
            evidence_ref: evidence_ref.into(),
            verdict: verdict.into(),
        }
    }

    #[test]
    fn rr_24_self_review_not_independent() {
        assert!(validate("sol", "sol", &[], &[]).is_err());
    }

    #[test]
    fn rr_24_false_evidence_is_rejected() {
        assert!(validate(
            "sol",
            "qwen",
            &[issue("unknown", "verified")],
            &["answer-1".into()]
        )
        .is_err());
    }

    #[test]
    fn rr_24_model_verified_does_not_authorize_revision() {
        let model = issue("answer-1", "verified");
        // The model claims verified, but the evidence ref is out of scope for the host.
        assert_eq!(
            host_verify(&model, &["answer-2".into()], &[]),
            HostVerification::Unverified
        );
        assert_eq!(
            host_verify(&model, &["answer-1".into()], &[]),
            HostVerification::Verified
        );
        // A revoked evidence reference can never be host-verified even if the model said verified.
        assert_eq!(
            host_verify(&model, &["answer-1".into()], &["answer-1".into()]),
            HostVerification::Unverified
        );
        assert!(!revision_allowed(
            host_verified_count(&[model], &["answer-2".into()], &[]),
            0,
            1
        ));
    }

    #[test]
    fn rr_24_issue_shape_and_count_are_bounded() {
        let mut too_many = Vec::new();
        for index in 0..9 {
            too_many.push(issue(&format!("answer-{index}"), "unverified"));
        }
        assert!(validate("sol", "qwen", &too_many, &["answer-0".into()]).is_err());
        let mut bad_kind = issue("answer-1", "verified");
        bad_kind.kind = "guess".into();
        assert!(validate("sol", "qwen", &[bad_kind], &["answer-1".into()]).is_err());
    }

    #[test]
    fn rr_24_review_packet_exposes_no_actor_or_model_name() {
        let packet = ReviewRequest::new("answer".into(), vec!["answer-1".into()]).expect("packet");
        assert_eq!(
            serde_json::to_value(packet).expect("json"),
            serde_json::json!({"answer":"answer","evidenceRefs":["answer-1"]})
        );
    }

    #[test]
    fn rr_25_review_round_limit_requires_verified_issue() {
        assert!(revision_allowed(1, 0, 1));
        assert!(!revision_allowed(0, 0, 1));
        assert!(!revision_allowed(1, 1, 1));
    }
}
