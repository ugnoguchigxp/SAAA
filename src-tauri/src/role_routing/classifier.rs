//! Bounded, host-validated interpretation of an in-flight follow-up.
//!
//! A model may suggest a classification, but it never receives authority to select a target,
//! dispatch a tool, approve a premium actor, or accept an answer.  This module deliberately
//! exposes only a validated data value for the coordinator to turn into an event.
use serde::Deserialize;

const MAX_EVIDENCE_CHARS: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClassificationKind {
    Status,
    ConstraintUpdate,
    Cancel,
    AnswerChallenge,
    PremiumApproval,
    Unclear,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Classification {
    pub(crate) kind: ClassificationKind,
    pub(crate) evidence: String,
    pub(crate) target_root_id: Option<String>,
    pub(crate) confidence: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawClassification {
    kind: ClassificationKind,
    evidence: String,
    target_root_id: Option<String>,
    confidence: f64,
}

/// Parses one strict classifier reply. A target may only name the active root; an omitted target
/// is permitted for `unclear` so a timeout can be represented without manufacturing a target.
pub(crate) fn parse(
    value: &str,
    active_root_id: &str,
    allowed_target_ids: &[String],
) -> Result<Classification, String> {
    let raw: RawClassification =
        serde_json::from_str(value).map_err(|_| "Role classification is malformed".to_string())?;
    if !raw.confidence.is_finite() || !(0.0..=1.0).contains(&raw.confidence) {
        return Err("Role classification confidence is invalid".into());
    }
    if raw.evidence.trim().is_empty() || raw.evidence.chars().count() > MAX_EVIDENCE_CHARS {
        return Err("Role classification evidence is invalid".into());
    }
    let normalized_evidence = raw.evidence.to_ascii_lowercase();
    if raw.kind != ClassificationKind::Unclear
        && ["こんにちは", "こんばんは", "hello", "hi "]
            .iter()
            .any(|greeting| normalized_evidence.contains(greeting))
    {
        return Err("Role classification mixes a social greeting with an action".into());
    }
    if let Some(target) = raw.target_root_id.as_deref() {
        if target != active_root_id || !allowed_target_ids.iter().any(|id| id == target) {
            return Err("Role classification target is invalid".into());
        }
    } else if raw.kind != ClassificationKind::Unclear {
        return Err("Role classification target is required".into());
    }
    Ok(Classification {
        kind: raw.kind,
        evidence: raw.evidence,
        target_root_id: raw.target_root_id,
        confidence: raw.confidence,
    })
}

/// Timeout is intentionally non-actionable.  The coordinator must ask for clarification rather
/// than treating a missing classifier result as cancellation or consent.
pub(crate) fn timeout() -> Classification {
    Classification {
        kind: ClassificationKind::Unclear,
        evidence: "classifier_timeout".into(),
        target_root_id: None,
        confidence: 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets() -> Vec<String> {
        vec!["root-1".into()]
    }

    #[test]
    fn rr_08_mixed_greeting_is_not_a_valid_action() {
        let input = r#"{"kind":"constraint_update","evidence":"こんにちは、条件も変更","targetRootId":"root-1","confidence":0.9}"#;
        assert_eq!(
            parse(input, "root-1", &targets()).unwrap_err(),
            "Role classification mixes a social greeting with an action"
        );
    }

    #[test]
    fn rr_08_bad_target_is_rejected() {
        let input = r#"{"kind":"cancel","evidence":"停止して","targetRootId":"other-root","confidence":1.0}"#;
        assert_eq!(
            parse(input, "root-1", &targets()).unwrap_err(),
            "Role classification target is invalid"
        );
    }

    #[test]
    fn rr_08_timeout_unclear() {
        assert_eq!(timeout().kind, ClassificationKind::Unclear);
        assert!(timeout().target_root_id.is_none());
    }
}
