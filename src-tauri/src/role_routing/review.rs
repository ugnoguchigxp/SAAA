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
    pub(crate) code: String,
    #[serde(rename = "evidenceRef")]
    pub(crate) evidence_ref: String,
    pub(crate) verdict: String,
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
