use super::*;
pub(super) fn is_weak_boundary(input: &str, cursor: usize, end: usize, grapheme: &str) -> bool {
    if grapheme.chars().all(char::is_whitespace) {
        return true;
    }
    if !matches!(grapheme, "、" | "," | ";" | ":" | "；" | "：") {
        return false;
    }
    let before_is_digit = input[..cursor]
        .chars()
        .next_back()
        .is_some_and(|value| value.is_ascii_digit());
    let after_is_digit = input[end..]
        .chars()
        .next()
        .is_some_and(|value| value.is_ascii_digit());
    !(before_is_digit && after_is_digit)
}
pub(super) fn is_safe_boundary(input: &str, end: usize) -> bool {
    if end >= input.len() {
        return true;
    }
    let before = input[..end].chars().next_back();
    let after = input[end..].chars().next();
    match (before, after) {
        (Some(left), Some(right))
            if left.is_ascii_alphanumeric() && right.is_ascii_alphanumeric() =>
        {
            false
        }
        (Some(left), Some(right)) if is_katakana(left) && is_katakana(right) => false,
        _ => true,
    }
}
pub(super) fn is_katakana(character: char) -> bool {
    matches!(character as u32, 0x30a0..=0x30ff | 0x31f0..=0x31ff | 0xff66..=0xff9d)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn completed(input: &str) -> SentenceAccumulator {
        let mut accumulator = SentenceAccumulator::default();
        accumulator.append(input).unwrap();
        accumulator.finish(input).unwrap();
        accumulator
    }

    #[test]
    fn waits_for_a_natural_first_sentence() {
        let mut accumulator = SentenceAccumulator::default();
        accumulator
            .append("短い。次の文は十分に長いです。")
            .unwrap();
        assert_eq!(
            accumulator.next_chunk(SelectReason::Append).unwrap().spoken,
            "短い。次の文は十分に長いです。"
        );
    }

    #[test]
    fn protects_urls_numbers_and_markdown() {
        let mut accumulator = completed("値は3.14です。 [資料](https://example.com/a) を確認してください。\n```rs\nlet x = 1;\n```");
        let first = accumulator.next_chunk(SelectReason::Completion).unwrap();
        assert_eq!(first.spoken, "値は3.14です。 資料 を確認してください。");
        assert!(accumulator.next_chunk(SelectReason::Completion).is_none());
    }

    #[test]
    fn idle_flushes_a_weak_boundary_after_minimum() {
        let mut accumulator = SentenceAccumulator::default();
        accumulator
            .append("これは十分に長い途中の文章ですが、")
            .unwrap();
        assert!(accumulator.next_chunk(SelectReason::Append).is_none());
        assert_eq!(
            accumulator.next_chunk(SelectReason::Idle).unwrap().spoken,
            "これは十分に長い途中の文章ですが、"
        );
    }

    #[test]
    fn completion_flushes_a_short_tail_once() {
        let mut accumulator = completed("十分に長い最初の文です。末尾");
        assert_eq!(
            accumulator
                .next_chunk(SelectReason::Completion)
                .unwrap()
                .spoken,
            "十分に長い最初の文です。"
        );
        assert_eq!(
            accumulator
                .next_chunk(SelectReason::Completion)
                .unwrap()
                .spoken,
            "末尾"
        );
        assert!(accumulator.next_chunk(SelectReason::Completion).is_none());
        assert!(accumulator.is_drained());
    }

    #[test]
    fn final_content_must_extend_the_delta_prefix() {
        let mut accumulator = SentenceAccumulator::default();
        accumulator.append("途中まで").unwrap();
        assert_eq!(
            accumulator.finish("別の内容"),
            Err(AccumulatorError::SyncLost)
        );
        accumulator.finish("途中まで続き").unwrap();
        assert_eq!(
            accumulator
                .next_chunk(SelectReason::Completion)
                .unwrap()
                .spoken,
            "途中まで続き"
        );
    }

    #[test]
    fn sixty_four_thousand_one_character_deltas_keep_incremental_limit_accounting() {
        let started = std::time::Instant::now();
        let mut accumulator = SentenceAccumulator::default();
        for _ in 0..MAX_SOURCE_CHARS {
            accumulator
                .append("日")
                .expect("delta remains within limit");
        }
        assert_eq!(
            accumulator.append("日"),
            Err(AccumulatorError::SourceTooLarge)
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn forced_split_never_cuts_ascii_words() {
        let input = format!("{}。", "longword".repeat(40));
        let mut accumulator = completed(&input);
        let chunk = accumulator.next_chunk(SelectReason::Completion).unwrap();
        assert_eq!(chunk.spoken, input);
    }

    #[test]
    fn hard_limit_wins_over_a_distant_sentence_end() {
        let input = format!("{}。", "あ".repeat(HARD_MAX + 40));
        let mut accumulator = completed(&input);
        let chunk = accumulator.next_chunk(SelectReason::Completion).unwrap();
        assert!(chunk.spoken.graphemes(true).count() <= HARD_MAX);
        assert!(chunk.spoken.chars().all(|character| character == 'あ'));
    }
}
