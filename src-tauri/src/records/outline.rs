use super::contract::{Outline, OutlineItem};

pub(crate) enum OutlineKind {
    Html,
    Markdown,
    Code,
    Json,
    Plain,
}

pub(crate) fn build(kind: OutlineKind, readable_text: &str) -> Outline {
    let items = match kind {
        OutlineKind::Markdown | OutlineKind::Html => headings(readable_text),
        OutlineKind::Code | OutlineKind::Json => code_lines(readable_text),
        OutlineKind::Plain => paragraphs(readable_text),
    };
    let mut kept = Vec::new();
    let mut bytes = 0usize;
    for item in items {
        let next = item.text.len();
        if kept.len() == 20 || bytes + next > 2_048 {
            break;
        }
        bytes += next;
        kept.push(item);
    }
    Outline {
        parser_version: "outline-v1",
        items: kept,
        bytes,
    }
}

fn headings(text: &str) -> Vec<OutlineItem> {
    let lines: Vec<&str> = text.lines().collect();
    let mut items = Vec::new();
    let mut offset = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let heading = line.trim_start();
        let marks = heading.split_whitespace().next().unwrap_or("");
        let is_md = !marks.is_empty()
            && marks.chars().all(|ch| ch == '#')
            && (1..=6).contains(&marks.len())
            && heading.len() > marks.len();
        let lower = heading.to_ascii_lowercase();
        let is_html = (1..=6).any(|level| lower.starts_with(&format!("<h{level}")));
        if is_md || is_html {
            let mut rendered = heading.to_string();
            if let Some(next) = lines.get(index + 1).filter(|line| !line.trim().is_empty()) {
                rendered.push(' ');
                rendered.push_str(next.trim());
            }
            items.push(OutlineItem {
                start_byte: offset as u64,
                text: rendered,
            });
        }
        offset += line.len() + 1;
    }
    items
}

fn code_lines(text: &str) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut offset = 0usize;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('{')
            || trimmed.starts_with('}')
            || trimmed.starts_with("fn ")
            || trimmed.starts_with("class ")
        {
            items.push(OutlineItem {
                start_byte: offset as u64,
                text: line.trim().to_string(),
            });
        }
        offset += line.len() + 1;
    }
    items
}

fn paragraphs(text: &str) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut offset = 0usize;
    for paragraph in text.split("\n\n") {
        let head: String = paragraph.chars().take(80).collect();
        if !head.trim().is_empty() {
            items.push(OutlineItem {
                start_byte: offset as u64,
                text: head,
            });
        }
        offset += paragraph.len() + 2;
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cw_26_markdown_outline_uses_headings() {
        let outline = build(
            OutlineKind::Markdown,
            "# Title\nIntro line\n\n## Next\nBody",
        );
        assert!(outline.items.iter().any(|item| item.text.contains("Title")));
        assert!(outline.items.iter().any(|item| item.text.contains("Intro")));
    }

    #[test]
    fn cw_26_plain_outline_paragraph_heads() {
        let outline = build(
            OutlineKind::Plain,
            "First paragraph stays.\n\nSecond starts here.",
        );
        assert_eq!(outline.items.len(), 2);
        assert!(outline.items[0].text.starts_with("First"));
        assert!(outline.items[1].text.starts_with("Second"));
    }

    #[test]
    fn cw_26_outline_caps_20_items_2048_bytes() {
        let text = (0..40)
            .map(|index| format!("# Heading number {index} with extra words to grow the line"))
            .collect::<Vec<_>>()
            .join("\n");
        let outline = build(OutlineKind::Markdown, &text);
        assert!(outline.items.len() <= 20);
        assert!(outline.bytes <= 2_048);
    }
}
