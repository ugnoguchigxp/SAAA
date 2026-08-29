use regex::Regex;
use std::sync::OnceLock;
use unicode_segmentation::UnicodeSegmentation;

const URL_BODY: &str = r#"[A-Za-z0-9._~:/?#@!$&'*+,;=%-]+"#;

pub(crate) fn text_for_speech(input: &str) -> String {
    let without_markdown_urls = markdown_link_regex().replace_all(input, "$1");
    let without_autolinks = autolink_regex().replace_all(&without_markdown_urls, "");
    let without_urls = url_regex().replace_all(&without_autolinks, "");
    let without_emoji = without_urls
        .graphemes(true)
        .filter(|grapheme| !is_emoji_grapheme(grapheme))
        .collect::<String>();
    without_emoji
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn markdown_link_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(&format!(
            r#"(?i)\[([^\]\r\n]{{1,512}})\]\(\s*(?:https?://|www\.){URL_BODY}\s*\)"#
        ))
        .expect("Markdown URL regex is valid")
    })
}

fn autolink_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(&format!(r#"(?i)<\s*(?:https?://|www\.){URL_BODY}\s*>"#))
            .expect("autolink URL regex is valid")
    })
}

fn url_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(&format!(r#"(?i)(?:https?://|www\.){URL_BODY}"#)).expect("URL regex is valid")
    })
}

fn is_emoji_grapheme(grapheme: &str) -> bool {
    grapheme.contains('\u{fe0f}')
        || grapheme.contains('\u{20e3}')
        || grapheme.chars().any(is_emoji_scalar)
}

fn is_emoji_scalar(character: char) -> bool {
    matches!(
        character as u32,
        0x1f000..=0x1faff
            | 0x231a..=0x231b
            | 0x23e9..=0x23f3
            | 0x23f8..=0x23fa
            | 0x25aa..=0x25ab
            | 0x25fb..=0x25fe
            | 0x2600..=0x26ff
            | 0x2700..=0x27bf
            | 0x2b1b..=0x2b1c
            | 0x2b50
            | 0x2b55
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_urls_without_consuming_adjacent_japanese_text() {
        assert_eq!(
            text_for_speech("詳細は https://example.com/docs?q=1 を確認してください。"),
            "詳細は を確認してください。"
        );
        assert_eq!(
            text_for_speech("詳細：https://example.com/docs。確認してください。"),
            "詳細：。確認してください。"
        );
        assert_eq!(
            text_for_speech("www.example.jp/path と <https://example.org/a>"),
            "と"
        );
    }

    #[test]
    fn keeps_markdown_link_labels_but_not_destinations() {
        assert_eq!(
            text_for_speech("[公式サイト](https://example.com/docs)で確認してください。"),
            "公式サイトで確認してください。"
        );
    }

    #[test]
    fn removes_emoji_sequences_and_normalizes_whitespace() {
        assert_eq!(
            text_for_speech("こんにちは 😊！ 🚀 家族👨‍👩‍👧‍👦 1️⃣ 🇯🇵"),
            "こんにちは ！ 家族"
        );
    }

    #[test]
    fn preserves_japanese_punctuation_and_normal_symbols() {
        let input = "通常記号: A+B=3、矢印→、括弧（保持）。";
        assert_eq!(text_for_speech(input), input);
    }

    #[test]
    fn url_and_emoji_only_input_becomes_empty() {
        assert!(text_for_speech("https://example.com 😊").is_empty());
    }
}
