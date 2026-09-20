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
}
