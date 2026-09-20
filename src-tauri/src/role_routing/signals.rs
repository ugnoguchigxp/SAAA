//! Conservative host-side interpretation of a follow-up while a root is active.
//! It produces a request for orchestration; it cannot itself dispatch tools or approve cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SignalKind {
    Status,
    ConstraintUpdate,
    Cancel,
    AnswerChallenge,
    PremiumApproval,
    Unclear,
}

/// Converts a provider's structured classification into the same deliberately limited signal
/// vocabulary as the conservative local fallback.  Any malformed or unauthorised result becomes
/// `Unclear`; callers must not infer cancellation, approval, or a tool operation from it.
pub(crate) fn classify_structured_follow_up(
    value: &str,
    active_root_id: &str,
    allowed_target_ids: &[String],
) -> SignalKind {
    let Ok(classification) =
        crate::role_routing::classifier::parse(value, active_root_id, allowed_target_ids)
    else {
        return SignalKind::Unclear;
    };
    let crate::role_routing::classifier::Classification {
        kind,
        evidence: _,
        target_root_id: _,
        confidence: _,
    } = classification;
    match kind {
        crate::role_routing::classifier::ClassificationKind::Status => SignalKind::Status,
        crate::role_routing::classifier::ClassificationKind::ConstraintUpdate => {
            SignalKind::ConstraintUpdate
        }
        crate::role_routing::classifier::ClassificationKind::Cancel => SignalKind::Cancel,
        crate::role_routing::classifier::ClassificationKind::AnswerChallenge => {
            SignalKind::AnswerChallenge
        }
        crate::role_routing::classifier::ClassificationKind::PremiumApproval => {
            SignalKind::PremiumApproval
        }
        crate::role_routing::classifier::ClassificationKind::Unclear => SignalKind::Unclear,
    }
}

pub(crate) fn classify_follow_up(input: &str) -> SignalKind {
    let text = input.trim();
    if text.is_empty() {
        return SignalKind::Unclear;
    }
    if ["停止", "やめて", "キャンセル", "cancel"]
        .iter()
        .any(|x| text.eq_ignore_ascii_case(x) || text.contains(x))
    {
        return SignalKind::Cancel;
    }
    // An approval needs a positive, explicit approval phrase. Mentioning Sol/Astra is not consent.
    if (text.contains("Sol") || text.contains("Astra"))
        && ["使って", "利用して", "承認", "お願いします"]
            .iter()
            .any(|x| text.contains(x))
    {
        return SignalKind::PremiumApproval;
    }
    if ["途中経過", "状況", "どうな", "考えていますか"]
        .iter()
        .any(|x| text.contains(x))
    {
        return SignalKind::Status;
    }
    if ["条件", "追加", "代わりに", "ただし", "変更"]
        .iter()
        .any(|x| text.contains(x))
    {
        return SignalKind::ConstraintUpdate;
    }
    // A quotation of an answer is not feedback by itself. Require a direct challenge marker.
    if !text.starts_with('「')
        && ["本当に", "違う", "おかしい", "再考", "根拠"]
            .iter()
            .any(|x| text.contains(x))
    {
        return SignalKind::AnswerChallenge;
    }
    SignalKind::Unclear
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rr_08_quote_not_feedback() {
        assert_eq!(
            classify_follow_up("「本当に正しい」ですか"),
            SignalKind::Unclear
        );
    }
    #[test]
    fn rr_16_status_does_not_cancel() {
        assert_eq!(classify_follow_up("途中経過を教えて"), SignalKind::Status);
    }
    #[test]
    fn rr_21_premium_requires_explicit_approval() {
        assert_eq!(
            classify_follow_up("Solなら良い答えですか"),
            SignalKind::Unclear
        );
        assert_eq!(
            classify_follow_up("Solを使ってお願いします"),
            SignalKind::PremiumApproval
        );
    }

    #[test]
    fn rr_08_structured_classification_is_fail_closed() {
        let targets = vec!["root-1".into()];
        assert_eq!(
            classify_structured_follow_up(
                r#"{"kind":"status","evidence":"進捗を聞いている","targetRootId":"root-1","confidence":0.9}"#,
                "root-1",
                &targets,
            ),
            SignalKind::Status
        );
        assert_eq!(
            classify_structured_follow_up(
                r#"{"kind":"cancel","evidence":"停止","targetRootId":"other","confidence":1.0}"#,
                "root-1",
                &targets,
            ),
            SignalKind::Unclear
        );
    }
}
