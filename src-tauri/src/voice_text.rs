use regex::Regex;
use std::sync::OnceLock;
use unicode_segmentation::UnicodeSegmentation;

const URL_BODY: &str = r#"[A-Za-z0-9._~:/?#@!$&'*+,;=%-]+"#;

pub(crate) fn text_for_speech(input: &str) -> String {
    let without_code = fenced_code_regex().replace_all(input, " ");
    let without_images = markdown_image_regex().replace_all(&without_code, "$1");
    let without_markdown_urls = markdown_link_regex().replace_all(&without_images, "$1");
    let without_inline_code = inline_code_regex().replace_all(&without_markdown_urls, "$1");
    let without_line_markers = markdown_line_marker_regex().replace_all(&without_inline_code, "");
    let without_emphasis = markdown_emphasis_regex().replace_all(&without_line_markers, "");
    let without_autolinks = autolink_regex().replace_all(&without_emphasis, "");
    let without_html = html_tag_regex().replace_all(&without_autolinks, " ");
    let without_urls = url_regex().replace_all(&without_html, "");
    let without_emoji = without_urls
        .graphemes(true)
        .filter(|grapheme| !is_emoji_grapheme(grapheme))
        .collect::<String>();
    without_emoji
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn fenced_code_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX
        .get_or_init(|| Regex::new(r"(?s)```.*?```|~~~.*?~~~").expect("fenced code regex is valid"))
}

fn markdown_image_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"!\[([^\]\r\n]{0,512})\]\([^\)\r\n]{0,2048}\)"#)
            .expect("markdown image regex is valid")
    })
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

fn inline_code_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"`{1,2}([^`\r\n]+?)`{1,2}").expect("inline code regex is valid")
    })
}

fn markdown_line_marker_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?m)^[\t ]{0,3}(?:#{1,6}[\t ]+|>[\t ]?|[-+*][\t ]+|\d+[.)][\t ]+)")
            .expect("markdown line marker regex is valid")
    })
}

fn markdown_emphasis_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"[*_~]{1,3}").expect("markdown emphasis regex is valid"))
}

fn autolink_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(&format!(r#"(?i)<\s*(?:https?://|www\.){URL_BODY}\s*>"#))
            .expect("autolink URL regex is valid")
    })
}

fn html_tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?s)<[^>]{0,2048}>").expect("html tag regex is valid"))
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
    matches!(character as u32, 0x1f000..=0x1faff | 0x231a..=0x231b | 0x23e9..=0x23f3 | 0x23f8..=0x23fa | 0x25aa..=0x25ab | 0x25fb..=0x25fe | 0x2600..=0x26ff | 0x2700..=0x27bf | 0x2b1b..=0x2b1c | 0x2b50 | 0x2b55)
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
    fn keeps_markdown_labels_and_removes_emoji_and_urls() {
        assert_eq!(
            text_for_speech("[公式サイト](https://example.com/docs)で確認してください。"),
            "公式サイトで確認してください。"
        );
        assert_eq!(
            text_for_speech("こんにちは 😊！ 🚀 家族👨‍👩‍👧‍👦 1️⃣ 🇯🇵"),
            "こんにちは ！ 家族"
        );
        assert!(text_for_speech("https://example.com 😊").is_empty());
    }

    #[test]
    fn projects_markdown_with_a_stable_chunk_safe_contract() {
        let symbols = "通常記号: A+B=3、矢印→、括弧（保持）。";
        assert_eq!(text_for_speech(symbols), symbols);
        assert_eq!(
            text_for_speech("例です。\n```rs\nlet secret = 1;\n```\n以上です。"),
            "例です。 以上です。"
        );
        assert_eq!(text_for_speech("Example:\n~~~rs\nhidden\n~~~"), "Example:");
        assert_eq!(
            text_for_speech("## 結論\n- ![図](https://example.com/image.png) [資料](https://example.com/docs) と `code` <b>確認</b>"),
            "結論 図 資料 と code 確認"
        );
    }

    #[test]
    fn projection_is_idempotent() {
        let input = "## 見出し\n[資料](https://example.com) と **強調**、`code`、😊";
        let projected = text_for_speech(input);
        assert_eq!(text_for_speech(&projected), projected);
    }
}
