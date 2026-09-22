//! Drops microphone copies of native TTS. Browser echoCancellation has no far-end
//! reference when playback happens outside the WebView.
use serde_json::Value;

pub(crate) fn is_self_speech_echo(heard: &str, spoken: &str) -> bool {
    let heard = normalize(heard);
    let spoken = normalize(spoken);
    let heard_len = heard.chars().count();
    if heard_len < 12 || spoken.chars().count() < 12 {
        return false;
    }
    let shared = longest_common_substring_len(&heard, &spoken);
    shared >= 12 && shared * 100 / heard_len >= 55
}

/// Keeps the part of `say` that is not a repeat of the previous assistant turn.
pub(crate) fn without_repeated_opening(previous: &str, say: &str) -> String {
    let kept = sentences(say)
        .into_iter()
        .filter(|sentence| !is_self_speech_echo(sentence, previous))
        .collect::<Vec<_>>()
        .join("");
    let kept = kept.trim();
    if kept == say.trim() {
        return say.to_string();
    }
    if kept.chars().count() >= 2 {
        kept.to_string()
    } else {
        "はい。".into()
    }
}

pub(crate) fn already_greeted(history: &[Value]) -> bool {
    history.iter().any(|message| message["role"] == "assistant")
}

fn sentences(text: &str) -> Vec<String> {
    let mut current = String::new();
    let mut out = Vec::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '。' | '！' | '？' | '!' | '?' | '\n') {
            push_sentence(&mut out, &current);
            current.clear();
        }
    }
    push_sentence(&mut out, &current);
    out
}

fn push_sentence(out: &mut Vec<String>, current: &str) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
}

fn normalize(text: &str) -> String {
    text.chars()
        .filter(|ch| !ch.is_whitespace() && !is_punctuation(*ch))
        .collect()
}

fn is_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '、' | '。'
            | '！'
            | '？'
            | '!'
            | '?'
            | '，'
            | '．'
            | '…'
            | '・'
            | '「'
            | '」'
            | '『'
            | '』'
            | ','
            | '.'
            | '〜'
            | '~'
    )
}

fn longest_common_substring_len(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous = vec![0usize; right.len() + 1];
    let mut best = 0usize;
    for left_ch in &left {
        let mut current = vec![0usize; right.len() + 1];
        for (index, right_ch) in right.iter().enumerate() {
            if left_ch == right_ch {
                current[index + 1] = previous[index] + 1;
                best = best.max(current[index + 1]);
            }
        }
        previous = current;
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENING: &str = "こんにちは、ゆうじさん！今日は何かお手伝いできることはありますか？";

    #[test]
    fn tts_mishearing_is_self_speech() {
        let heard = "よう、ミュージさん、今日は何かお手伝いできることはありますか？";
        let spoken = "おはよう、ゆうじさん。今日は何かお手伝いできることはありますか？";
        assert!(is_self_speech_echo(heard, spoken));
        assert!(is_self_speech_echo(spoken, OPENING));
    }

    #[test]
    fn a_short_greeting_is_not_self_speech() {
        assert!(!is_self_speech_echo("おはよう。", OPENING));
        assert!(!is_self_speech_echo("おはよう、ゆうじさん。", OPENING));
    }

    #[test]
    fn repeated_help_question_is_removed_and_the_reply_to_the_user_stays() {
        let say = "おはよう、ゆうじさん。今日は何かお手伝いできることはありますか？";
        assert_eq!(
            without_repeated_opening(OPENING, say),
            "おはよう、ゆうじさん。"
        );
    }

    #[test]
    fn a_reply_that_only_repeats_the_opening_becomes_a_short_acknowledgement() {
        assert_eq!(
            without_repeated_opening(
                OPENING,
                "こんにちは、ゆうじさん。今日は何かお手伝いできることはありますか？"
            ),
            "こんにちは、ゆうじさん。"
        );
        assert_eq!(
            without_repeated_opening(OPENING, "今日は何かお手伝いできることはありますか？"),
            "はい。"
        );
    }
}
