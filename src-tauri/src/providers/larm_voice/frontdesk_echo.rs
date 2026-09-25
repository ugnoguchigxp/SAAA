//! Drops microphone copies of native TTS. Browser echoCancellation has no far-end
//! reference when playback happens outside the WebView.
pub(crate) fn is_self_speech_echo(heard: &str, spoken: &str, aec_active: bool) -> bool {
    let heard = normalize(heard);
    let spoken = normalize(spoken);
    let heard_len = heard.chars().count();
    let min_shared = if aec_active { 24 } else { 12 };
    let min_ratio = if aec_active { 75 } else { 55 };
    if heard_len < min_shared || spoken.chars().count() < min_shared {
        return false;
    }
    let shared = longest_common_substring_len(&heard, &spoken);
    shared >= min_shared && shared * 100 / heard_len >= min_ratio
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
        assert!(is_self_speech_echo(heard, spoken, false));
        assert!(is_self_speech_echo(spoken, OPENING, false));
        assert!(!is_self_speech_echo(heard, spoken, true));
    }

    #[test]
    fn a_short_greeting_is_not_self_speech() {
        assert!(!is_self_speech_echo("おはよう。", OPENING, false));
        assert!(!is_self_speech_echo(
            "おはよう、ゆうじさん。",
            OPENING,
            false
        ));
    }
}
