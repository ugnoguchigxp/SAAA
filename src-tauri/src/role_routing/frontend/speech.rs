#[cfg(any(test, feature = "offline-contracts"))]
use super::*;

#[cfg(any(test, feature = "offline-contracts"))]
/// Keep a short, underspecified speech request in the frontend, and do not let
/// a nod close a complete request that the small model failed to classify.
pub(crate) fn guard_for_input(mut result: FrontendResult, input: &str) -> FrontendResult {
    if is_bare_speech_request(input) {
        result.kind = FrontendKind::Answer;
        result.reply = "何を読み上げましょうか？".to_string();
        return result;
    }
    if result.kind == FrontendKind::Nod && is_explicit_request(input) {
        result.kind = FrontendKind::Handoff;
        result.reply = WAIT_LINE.to_string();
    }
    result
}

#[cfg(any(test, feature = "offline-contracts"))]
fn is_bare_speech_request(input: &str) -> bool {
    let input = input.trim().trim_end_matches(['。', '！', '!', '？', '?']);
    [
        "発声して",
        "発声してください",
        "発生して",
        "発生してください",
        "読んで",
        "読んでください",
        "話して",
        "話してください",
    ]
    .contains(&input)
}

#[cfg(any(test, feature = "offline-contracts"))]
fn is_explicit_request(input: &str) -> bool {
    let input = input.trim();
    input.contains('？')
        || input.contains('?')
        || [
            "ください",
            "教えて",
            "調べて",
            "説明して",
            "読んで",
            "話して",
            "発声して",
        ]
        .iter()
        .any(|marker| input.contains(marker))
}

#[cfg(any(test, feature = "offline-contracts"))]
/// The receptionist's own sentence. Empty or multi-line text is not spoken.
pub(crate) fn spoken_line(result: &FrontendResult) -> Option<String> {
    if result.kind == FrontendKind::Handoff {
        let reply = result.reply.trim();
        return Some(if [THINK_LINE, SEARCH_LINE, WAIT_LINE].contains(&reply) {
            reply.to_string()
        } else {
            WAIT_LINE.to_string()
        });
    }
    let text = strip_leading_stage_tag(result.reply.trim());
    if text.is_empty() || text.contains('\n') || text.chars().count() > 80 {
        return None;
    }
    Some(text.to_string())
}

#[cfg(any(test, feature = "offline-contracts"))]
fn strip_leading_stage_tag(text: &str) -> &str {
    let Some(rest) = text.strip_prefix('[') else {
        return text;
    };
    let Some(end) = rest.find(']') else {
        return text;
    };
    let tag = &rest[..end];
    if tag.is_empty() || tag.chars().count() > 24 || tag.contains('\n') {
        return text;
    }
    rest[end + 1..].trim_start()
}

#[cfg(test)]
pub(crate) fn record_filler_tick(tick: u32) {
    filler_ticks().lock().unwrap().push(tick);
}

#[cfg(test)]
pub(crate) fn take_filler_ticks() -> Vec<u32> {
    std::mem::take(&mut *filler_ticks().lock().unwrap())
}

#[cfg(test)]
fn filler_ticks() -> &'static std::sync::Mutex<Vec<u32>> {
    static TICKS: std::sync::OnceLock<std::sync::Mutex<Vec<u32>>> = std::sync::OnceLock::new();
    TICKS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

#[cfg(any(test, feature = "offline-contracts"))]
/// `(speak, defer_to_next_tick)`. Tick 20 is the caller's timeout and never speaks.
pub(crate) fn filler_decision(tick: u32, playing: bool, deferred: bool) -> (bool, bool) {
    if tick == 0 || tick >= 20 {
        return (false, false);
    }
    if deferred {
        return (!playing, false);
    }
    if tick % 5 != 0 {
        return (false, false);
    }
    if playing {
        return (false, true);
    }
    (true, false)
}
