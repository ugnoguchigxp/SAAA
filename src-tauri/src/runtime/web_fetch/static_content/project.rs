//! Convert page structure to short evidence text without sending HTML to the model.
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashSet, sync::OnceLock};

const MAX_BLOCK_CHARS: usize = 160;
const MAX_OUTPUT_CANDIDATES: usize = 64;

#[derive(Debug, Clone)]
struct Block {
    text: String,
    heading: String,
    in_main: bool,
}

#[derive(Debug, Clone)]
pub(super) struct Projection {
    pub text: String,
    pub relevant: bool,
    pub truncated: bool,
    pub has_body: bool,
}

fn normalize(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn element_text(element: ElementRef<'_>) -> String {
    normalize(&element.text().collect::<Vec<_>>().join(" "))
}

fn excluded(element: ElementRef<'_>) -> bool {
    std::iter::once(element)
        .chain(element.ancestors().filter_map(ElementRef::wrap))
        .any(|ancestor| {
            let name = ancestor.value().name();
            if matches!(
                name,
                "script"
                    | "style"
                    | "noscript"
                    | "nav"
                    | "footer"
                    | "aside"
                    | "template"
                    | "svg"
                    | "iframe"
            ) {
                return true;
            }
            let value = ancestor.value();
            value.attr("hidden").is_some()
                || value.attr("inert").is_some()
                || value.attr("aria-hidden") == Some("true")
                || value.attr("style").is_some_and(|style| {
                    let compact = style.to_ascii_lowercase().replace(' ', "");
                    compact.contains("display:none") || compact.contains("visibility:hidden")
                })
        })
}

fn title(document: &Html) -> String {
    let selector = Selector::parse("title").expect("static selector");
    document
        .select(&selector)
        .next()
        .map(element_text)
        .unwrap_or_default()
}

fn push_block(blocks: &mut Vec<Block>, text: String, heading: &str, in_main: bool) {
    let text = normalize(&text);
    if text.is_empty() {
        return;
    }
    let chars = text.chars().collect::<Vec<_>>();
    let mut offset = 0;
    while offset < chars.len() {
        let end = (offset + MAX_BLOCK_CHARS).min(chars.len());
        let part = chars[offset..end].iter().collect::<String>();
        blocks.push(Block {
            text: part,
            heading: heading.to_string(),
            in_main,
        });
        if end == chars.len() {
            break;
        }
        offset = end.saturating_sub(40);
    }
}

fn push_table_value(
    blocks: &mut Vec<Block>,
    label: &str,
    column: &str,
    value: &str,
    heading: &str,
    in_main: bool,
) {
    let prefix = format!("{} — {}: ", label, column);
    let capacity = MAX_BLOCK_CHARS.saturating_sub(prefix.chars().count());
    if capacity < 40 {
        push_block(blocks, format!("{prefix}{value}"), heading, in_main);
        return;
    }
    let chars = value.chars().collect::<Vec<_>>();
    let mut offset = 0;
    while offset < chars.len() {
        let end = (offset + capacity).min(chars.len());
        blocks.push(Block {
            text: format!(
                "{}{}",
                prefix,
                chars[offset..end].iter().collect::<String>()
            ),
            heading: heading.to_string(),
            in_main,
        });
        if end == chars.len() {
            break;
        }
        offset = end.saturating_sub(40.min(capacity / 2));
    }
}

fn table_rows(table: ElementRef<'_>, heading: &str, in_main: bool, blocks: &mut Vec<Block>) {
    let row_selector = Selector::parse("tr").expect("static selector");
    let cell_selector = Selector::parse("th,td").expect("static selector");
    let mut headers: Vec<String> = Vec::new();
    let mut group = String::new();
    for row in table.select(&row_selector) {
        if excluded(row) {
            continue;
        }
        let cells = row
            .select(&cell_selector)
            .filter(|cell| !excluded(*cell))
            .map(element_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>();
        if cells.is_empty() {
            continue;
        }
        if cells.len() == 1
            && !has_value(&cells[0])
            && row
                .select(&cell_selector)
                .any(|cell| cell.value().attr("colspan").is_some())
        {
            if cells[0].chars().count() <= 60 {
                group = cells[0].clone();
            }
            continue;
        }
        let all_headers = row
            .select(&cell_selector)
            .all(|cell| cell.value().name() == "th");
        let period_header = cells.iter().skip(1).any(|cell| {
            let lower = cell.to_ascii_lowercase();
            lower.contains("fy")
                || lower.contains("quarter")
                || lower.contains("q1")
                || lower.contains("q2")
                || lower.contains("q3")
                || lower.contains("q4")
        });
        if (all_headers || period_header)
            && cells.len() > 1
            && !cells.iter().skip(1).any(|cell| has_value(cell))
        {
            headers = cells;
            continue;
        }
        let columns = if headers.len() == cells.len() && cells.len() > 1 {
            Some(headers.iter().skip(1).collect::<Vec<_>>())
        } else if headers.len() + 1 == cells.len() && cells.len() > 1 {
            Some(headers.iter().collect::<Vec<_>>())
        } else {
            None
        };
        if let Some(columns) = columns {
            for (value, column) in cells.iter().skip(1).zip(columns) {
                let label = if group.is_empty() {
                    cells[0].clone()
                } else {
                    format!("{} — {}", group, cells[0])
                };
                push_table_value(blocks, &label, column, value, heading, in_main);
            }
        } else {
            // Ambiguous spanning cells: preserve row order without inventing associations.
            let text = if group.is_empty() {
                cells.join(" | ")
            } else {
                format!("{} — {}", group, cells.join(" | "))
            };
            push_block(blocks, text, heading, in_main);
        }
    }
}

fn collect(document: &Html) -> (String, Vec<Block>) {
    let title = title(document);
    let selector =
        Selector::parse("h1,h2,h3,h4,p,li,blockquote,table,dl,div,span").expect("static selector");
    let main_selector = Selector::parse("main,article,[role=main]").expect("static selector");
    let body_selector = Selector::parse("body").expect("static selector");
    let Some(body) = document.select(&body_selector).next() else {
        return (title, Vec::new());
    };
    let main_nodes = document
        .select(&main_selector)
        .map(|node| node.id())
        .collect::<HashSet<_>>();
    let mut blocks = Vec::new();
    let mut heading = String::new();
    for element in body.select(&selector) {
        if excluded(element) {
            continue;
        }
        let name = element.value().name();
        let in_main = element
            .ancestors()
            .any(|node| main_nodes.contains(&node.id()));
        if name == "table" {
            table_rows(element, &heading, in_main, &mut blocks);
            continue;
        }
        if element
            .ancestors()
            .filter_map(ElementRef::wrap)
            .any(|parent| parent.value().name() == "table")
        {
            continue;
        }
        if matches!(name, "div" | "span" | "dl") {
            if element
                .ancestors()
                .filter_map(ElementRef::wrap)
                .any(|parent| {
                    matches!(
                        parent.value().name(),
                        "p" | "li" | "blockquote" | "h1" | "h2" | "h3" | "h4"
                    )
                })
            {
                continue;
            }
            let has_structural_child =
                element
                    .children()
                    .filter_map(ElementRef::wrap)
                    .any(|child| {
                        matches!(
                            child.value().name(),
                            "div" | "span" | "p" | "li" | "table" | "dl"
                        )
                    });
            if has_structural_child {
                continue;
            }
        }
        let text = element_text(element);
        if matches!(name, "h1" | "h2" | "h3" | "h4") {
            if !text.is_empty() {
                heading = text;
            }
            continue;
        }
        if name == "li"
            && element
                .children()
                .filter_map(ElementRef::wrap)
                .any(|child| child.value().name() == "p")
        {
            continue;
        }
        push_block(&mut blocks, text, &heading, in_main);
    }
    (title, blocks)
}

fn query_terms(query: &str) -> Vec<String> {
    let mut normalized_query = query
        .to_lowercase()
        .replace("non-gaap", "nongaap")
        .replace("non gaap", "nongaap");
    for (phrase, canonical) in [
        ("first quarter", "q1"),
        ("second quarter", "q2"),
        ("third quarter", "q3"),
        ("fourth quarter", "q4"),
        ("第1四半期", "q1"),
        ("第2四半期", "q2"),
        ("第3四半期", "q3"),
        ("第4四半期", "q4"),
    ] {
        normalized_query = normalized_query.replace(phrase, canonical);
    }
    static FISCAL_YEAR: OnceLock<regex::Regex> = OnceLock::new();
    normalized_query = FISCAL_YEAR
        .get_or_init(|| regex::Regex::new(r"fiscal(?: year)?\s+(\d{4})").expect("fiscal year"))
        .replace_all(&normalized_query, "fy$1")
        .into_owned();
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut ascii = None;
    for c in normalized_query.chars().chain(std::iter::once(' ')) {
        let kind = if c.is_ascii_alphanumeric() {
            Some(true)
        } else if c.is_alphanumeric() {
            Some(false)
        } else {
            None
        };
        if (kind != ascii || kind.is_none()) && !current.is_empty() {
            if ascii == Some(true) {
                let term = current.to_ascii_lowercase();
                if term.len() >= 2
                    && !matches!(
                        term.as_str(),
                        "what"
                            | "when"
                            | "where"
                            | "which"
                            | "the"
                            | "and"
                            | "for"
                            | "latest"
                            | "about"
                            | "how"
                            | "much"
                            | "please"
                    )
                {
                    terms.push(term);
                }
            } else {
                let chars = current.chars().collect::<Vec<_>>();
                for pair in chars.windows(2) {
                    let term = pair.iter().collect::<String>();
                    if !term.chars().any(|c| matches!(c, 'の' | 'は' | 'を' | 'が'))
                        && !matches!(
                            term.as_str(),
                            "です"
                                | "ます"
                                | "して"
                                | "教え"
                                | "えて"
                                | "とは"
                                | "につ"
                                | "つい"
                                | "いて"
                                | "いく"
                                | "くら"
                                | "何で"
                        )
                    {
                        terms.push(term);
                    }
                }
            }
            current.clear();
        }
        if let Some(next) = kind {
            current.push(c);
            ascii = Some(next);
        } else {
            ascii = None;
        }
    }
    terms.sort();
    terms.dedup();
    terms
}

fn has_value(text: &str) -> bool {
    static VALUE: OnceLock<regex::Regex> = OnceLock::new();
    VALUE.get_or_init(|| regex::Regex::new(
        r"(?i)(?:[$€¥]\s*[0-9][0-9,.]*|[0-9][0-9,.]*\s*(?:billion|million|trillion|bn|mn|億|兆|円|ドル|%)|\b[0-9]{1,3}(?:,[0-9]{3})+(?:\.[0-9]+)?\b|\b[0-9]+\.[0-9]+\b)"
    ).expect("static value pattern")).is_match(text)
}

fn needs_value(query: &str) -> bool {
    let lower = query.to_lowercase();
    [
        "revenue",
        "price",
        "profit",
        "earnings",
        "income",
        "sales",
        "amount",
        "market cap",
        "how much",
        "売上",
        "株価",
        "価格",
        "利益",
        "いくら",
        "何ドル",
        "何円",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

fn term_matches(text: &str, term: &str) -> bool {
    if term == "nongaap" {
        return text.contains("non-gaap") || text.contains("non gaap") || text.contains("nongaap");
    }
    if term == "gaap" {
        return text.match_indices("gaap").any(|(index, _)| {
            let prefix = &text[..index];
            !prefix.ends_with("non-") && !prefix.ends_with("non ") && !prefix.ends_with("non")
        });
    }
    if text.contains(term) {
        return true;
    }
    if let Some(year) = term.strip_prefix("fy") {
        if year.len() == 4 && year.chars().all(|c| c.is_ascii_digit()) {
            let short = &year[2..];
            return text.contains(&format!("fiscal {year}"))
                || text.contains(&format!("fiscal year {year}"))
                || text.contains(&format!("fy{short}"))
                || text.contains(&format!("fiscal {short}"));
        }
    }
    if let Some(quarter) = term.strip_prefix('q') {
        return match quarter {
            "1" => ["first quarter", "1st quarter", "quarter 1", "第1四半期"]
                .iter()
                .any(|alias| text.contains(alias)),
            "2" => ["second quarter", "2nd quarter", "quarter 2", "第2四半期"]
                .iter()
                .any(|alias| text.contains(alias)),
            "3" => ["third quarter", "3rd quarter", "quarter 3", "第3四半期"]
                .iter()
                .any(|alias| text.contains(alias)),
            "4" => ["fourth quarter", "4th quarter", "quarter 4", "第4四半期"]
                .iter()
                .any(|alias| text.contains(alias)),
            _ => false,
        };
    }
    match term {
        "revenue" => ["sales", "売上高", "売上"]
            .iter()
            .any(|alias| text.contains(alias)),
        "price" => ["stock price", "株価", "価格"]
            .iter()
            .any(|alias| text.contains(alias)),
        _ => false,
    }
}

fn metric_term(term: &str) -> bool {
    matches!(
        term,
        "revenue"
            | "sales"
            | "price"
            | "profit"
            | "earnings"
            | "income"
            | "amount"
            | "売上"
            | "上高"
            | "株価"
            | "価格"
            | "利益"
    )
}

fn conflicting_accounting_basis(text: &str, terms: &[String]) -> bool {
    let lower = text.to_lowercase();
    terms.iter().any(|term| term == "gaap")
        && !terms.iter().any(|term| term == "nongaap")
        && (lower.contains("non-gaap") || lower.contains("non gaap") || lower.contains("nongaap"))
}

fn conflicting_period(text: &str, terms: &[String]) -> bool {
    static QUARTER: OnceLock<regex::Regex> = OnceLock::new();
    static FISCAL: OnceLock<regex::Regex> = OnceLock::new();
    let quarter = QUARTER.get_or_init(|| {
        regex::Regex::new(r"(?i)\b(?:q[1-4]|(?:first|second|third|fourth) quarter)\b")
            .expect("quarter pattern")
    });
    let fiscal = FISCAL.get_or_init(|| {
        regex::Regex::new(r"(?i)\b(?:fy\s*\d{2,4}|fiscal(?: year)?\s+\d{2,4})\b")
            .expect("fiscal pattern")
    });
    (quarter.is_match(text)
        && terms
            .iter()
            .filter(|term| matches!(term.as_str(), "q1" | "q2" | "q3" | "q4"))
            .any(|term| !term_matches(text, term)))
        || (fiscal.is_match(text)
            && terms
                .iter()
                .filter(|term| {
                    term.starts_with("fy") && term[2..].chars().all(|c| c.is_ascii_digit())
                })
                .any(|term| !term_matches(text, term)))
}

fn choose(title: &str, blocks: &[Block], query: Option<&str>, limit: usize) -> Projection {
    let terms = query.map(query_terms).unwrap_or_default();
    let value_needed = query.is_some_and(needs_value);
    let title_lower = title.to_lowercase();
    let mut ranked = blocks
        .iter()
        .enumerate()
        .filter(|(_, block)| {
            !conflicting_accounting_basis(&block.text, &terms)
                && !conflicting_period(&block.text.to_lowercase(), &terms)
        })
        .map(|(index, block)| {
            let local = format!("{} {}", block.heading, block.text).to_lowercase();
            let matched = terms
                .iter()
                .filter(|term| term_matches(&local, term))
                .count();
            let covered = terms
                .iter()
                .filter(|term| term_matches(&local, term) || term_matches(&title_lower, term))
                .count();
            let block_text = block.text.to_lowercase();
            let metric_matched = terms
                .iter()
                .filter(|term| metric_term(term))
                .any(|term| term_matches(&block_text, term));
            let value = has_value(&block.text);
            let score = matched * 12
                + usize::from(block.in_main) * 2
                + usize::from(value && value_needed) * 4;
            (index, score, matched, covered, value, metric_matched)
        })
        .collect::<Vec<_>>();
    ranked.sort_by_key(|(index, score, _, _, _, _)| (std::cmp::Reverse(*score), *index));
    let best = ranked.first().copied();
    let relevant = if terms.is_empty() {
        false
    } else if let Some((_, _, matched, covered, value, metric_matched)) = best {
        let required = if terms.len() <= 2 {
            terms.len()
        } else {
            (terms.len() * 2).div_ceil(3)
        };
        matched > 0
            && covered >= required
            && (!value_needed
                || (value && (metric_matched || !terms.iter().any(|term| metric_term(term)))))
    } else {
        false
    };
    // Select answer-bearing candidates first. The character budget limits expansion.
    let mut selected = Vec::new();
    if terms.is_empty() {
        selected.extend(0..blocks.len().min(MAX_OUTPUT_CANDIDATES));
    } else if let Some((best_index, _, _, _, _, _)) = best {
        selected.push(best_index);
    }
    for (index, _, matched, _, _, _) in ranked.iter().copied() {
        if selected.len() >= MAX_OUTPUT_CANDIDATES {
            break;
        }
        if !terms.is_empty() && matched == 0 {
            continue;
        }
        if !selected.contains(&index) {
            selected.push(index);
        }
    }
    if selected.is_empty() && terms.is_empty() && !blocks.is_empty() {
        selected.push(0);
    }
    let mut out = String::new();
    let mut last_heading = String::new();
    let mut emitted_best = false;
    let title_budget = if limit <= 400 { 0 } else { 160 };
    let display_title = title.chars().take(title_budget).collect::<String>();
    let mut truncated = title.chars().count() > title_budget;
    out.push_str(&display_title);
    for index in selected {
        let block = &blocks[index];
        let prefix = if !block.heading.is_empty() && block.heading != last_heading {
            format!("\n{}\n", block.heading.chars().take(80).collect::<String>())
        } else {
            "\n".to_string()
        };
        let addition = format!("{}{}", prefix, block.text);
        if out.chars().count() + addition.chars().count() > limit {
            truncated = true;
            continue;
        }
        out.push_str(&addition);
        last_heading = block.heading.clone();
        if best.is_some_and(|(best_index, ..)| best_index == index) {
            emitted_best = true;
        }
    }
    if out.is_empty() && terms.is_empty() && !blocks.is_empty() {
        out = blocks[0].text.chars().take(limit).collect();
        truncated = blocks[0].text.chars().count() > limit;
    }
    Projection {
        text: out.trim().to_string(),
        relevant: relevant && emitted_best,
        truncated: truncated || blocks.len() > MAX_OUTPUT_CANDIDATES,
        has_body: !blocks.is_empty(),
    }
}

pub(super) fn html(html: &str, query: Option<&str>, limit: usize) -> Projection {
    let document = Html::parse_document(html);
    let (title, blocks) = collect(&document);
    choose(&title, &blocks, query, limit)
}

pub(super) fn plain(text: &str, query: Option<&str>, limit: usize) -> Projection {
    let mut blocks = Vec::new();
    for line in text.lines() {
        push_block(&mut blocks, line.to_string(), "", false);
    }
    if blocks.is_empty() {
        push_block(&mut blocks, text.to_string(), "", false);
    }
    choose("", &blocks, query, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn financial_table_keeps_period_and_metric_with_value() {
        let page = "<title>NVIDIA FY2026 Q2 results</title><main><table><tr><th>Metric</th><th>FY2026 Q2</th><th>FY2025 Q2</th></tr><tr><th>Revenue</th><td>$46.7 billion</td><td>$30.0 billion</td></tr></table></main>";
        let result = html(page, Some("NVIDIA revenue FY2026 Q2"), 500);
        assert!(result.relevant, "{result:?}");
        assert!(result.text.contains("FY2026 Q2: $46.7 billion"));
        assert!(!result.text.contains("FY2025 Q2: $30.0 billion"));
    }

    #[test]
    fn newsroom_td_headers_keep_gaap_period_and_value_together() {
        let page = "<title>NVIDIA Second Quarter Fiscal 2026</title><main><table><tr><td colspan='3'>GAAP</td></tr><tr><td>($ in millions)</td><td>Q2 FY26</td><td>Q1 FY26</td></tr><tr><td>Revenue</td><td>$46,743</td><td>$44,062</td></tr></table></main>";
        let result = html(page, Some("NVIDIA revenue FY2026 Q2"), 500);
        assert!(result.relevant, "{result:?}");
        assert!(
            result.text.contains("GAAP — Revenue — Q2 FY26: $46,743"),
            "{}",
            result.text
        );
        assert!(!result.text.contains("Q1 FY26: $44,062"));
    }

    #[test]
    fn short_table_value_is_not_dropped() {
        let page = "<title>Results</title><main><table><tr><th>Metric</th><th>Q2 FY26</th></tr><tr><td>Revenue</td><td>$5</td></tr></table></main>";
        let result = html(page, Some("Revenue Q2 FY26"), 200);
        assert!(result.relevant, "{result:?}");
        assert!(result.text.contains("Revenue — Q2 FY26: $5"));
    }

    #[test]
    fn value_in_spanning_cell_is_not_treated_as_group_header() {
        let page = "<main><table><tr><td colspan='2'>Revenue Q2 FY26: $5</td></tr></table></main>";
        let result = html(page, Some("Revenue Q2 FY26"), 200);
        assert!(result.relevant, "{result:?}");
        assert!(result.text.contains("Revenue Q2 FY26: $5"));
    }

    #[test]
    fn long_table_cell_keeps_label_on_later_value_chunk() {
        let page = format!(
            "<main><table><tr><th>Metric</th><th>Q2 FY26</th></tr><tr><td>Revenue</td><td>{} $5 billion</td></tr></table></main>",
            "background ".repeat(30)
        );
        let result = html(&page, Some("Revenue Q2 FY26"), 200);
        assert!(result.relevant, "{result:?}");
        assert!(result.text.contains("$5 billion"), "{result:?}");
        assert!(result.text.contains("Revenue — Q2 FY26:"));
    }

    #[test]
    fn other_period_in_title_does_not_validate_wrong_table_value() {
        let page = "<title>NVIDIA FY2026 Q2</title><main><table><tr><th>Metric</th><th>Q1 FY26</th></tr><tr><td>Revenue</td><td>$4 billion</td></tr></table></main>";
        let result = html(page, Some("NVIDIA revenue FY2026 Q2"), 200);
        assert!(!result.relevant, "{result:?}");
        assert!(!result.text.contains("$4 billion"), "{result:?}");
    }

    #[test]
    fn natural_language_period_filters_other_quarter() {
        let page = "<title>NVIDIA second quarter fiscal 2026</title><main><table><tr><th>Metric</th><th>Q1 FY26</th></tr><tr><td>Revenue</td><td>$4 billion</td></tr></table></main>";
        let result = html(page, Some("NVIDIA revenue second quarter fiscal 2026"), 200);
        assert!(!result.relevant, "{result:?}");
    }

    #[test]
    fn fiscal_year_in_title_does_not_validate_prior_year_value() {
        let page = "<title>NVIDIA fiscal 2026 results</title><main><p>Fiscal 2025 revenue was $4 billion.</p></main>";
        let result = html(page, Some("NVIDIA revenue fiscal 2026"), 200);
        assert!(!result.relevant, "{result:?}");
    }

    #[test]
    fn title_can_supply_year_when_body_specifies_matching_quarter() {
        let page = "<title>NVIDIA fiscal 2026 results</title><main><p>Q2 revenue was $5 billion.</p></main>";
        let result = html(page, Some("NVIDIA revenue fiscal 2026 Q2"), 200);
        assert!(result.relevant, "{result:?}");
    }

    #[test]
    fn gaap_query_does_not_choose_non_gaap_value() {
        let page = "<title>NVIDIA Q2 FY2026</title><main><table><tr><td colspan='2'>Non-GAAP</td></tr><tr><td>Metric</td><td>Q2 FY26</td></tr><tr><td>Revenue</td><td>$47,000</td></tr><tr><td colspan='2'>GAAP</td></tr><tr><td>Metric</td><td>Q2 FY26</td></tr><tr><td>Revenue</td><td>$46,743</td></tr></table></main>";
        let result = html(page, Some("NVIDIA GAAP revenue FY2026 Q2"), 180);
        assert!(result.relevant, "{result:?}");
        assert!(result.text.contains("$46,743"), "{}", result.text);
        assert!(!result.text.contains("$47,000"), "{}", result.text);
    }

    #[test]
    fn plain_text_without_query_keeps_multiple_lines() {
        let result = plain(
            "First paragraph.\nSecond paragraph.\nThird paragraph.",
            None,
            200,
        );
        assert!(result.text.contains("First paragraph."));
        assert!(result.text.contains("Second paragraph."));
        assert!(result.text.contains("Third paragraph."));
        assert!(!result.relevant);
    }

    #[test]
    fn brief_complete_page_has_body() {
        let result = html(
            "<title>Weather</title><main><p>Sunny.</p></main>",
            None,
            200,
        );
        assert!(result.has_body);
        assert!(result.text.contains("Sunny."));
    }

    #[test]
    fn long_title_never_exceeds_character_budget() {
        let page = format!(
            "<title>{}</title><main><p>Revenue Q2 FY26: $5</p></main>",
            "NVIDIA ".repeat(1_000)
        );
        let result = html(&page, Some("Revenue Q2 FY26"), 200);
        assert!(result.text.chars().count() <= 200);
        assert!(result.text.contains("$5"), "{result:?}");
        assert!(result.truncated);
    }

    #[test]
    fn fiscal_period_aliases_match_newsroom_headline() {
        let page = "<title>NVIDIA Announces Financial Results for Second Quarter Fiscal 2026</title><main><p>Revenue of $46.7 billion, up 6% from the prior quarter.</p></main>";
        let result = html(page, Some("NVIDIA revenue FY2026 Q2"), 500);
        assert!(result.relevant, "{result:?}");
        assert!(result.text.contains("$46.7 billion"));
    }

    #[test]
    fn subject_only_match_does_not_count_as_target_evidence() {
        let page = "<title>NVIDIA news</title><main><p>NVIDIA announced a new product and several software updates for developers around the world.</p></main>";
        assert!(!html(page, Some("NVIDIA revenue FY2026 Q2"), 500).relevant);
    }

    #[test]
    fn scans_beyond_first_thousand_blocks() {
        let mut page = String::from("<title>NVIDIA financial results</title><main>");
        for _ in 0..1_100 {
            page.push_str("<p>General product information for customers and developers.</p>");
        }
        page.push_str("<p>Revenue for FY2026 Q2: $46.7 billion.</p></main>");
        let result = html(&page, Some("NVIDIA revenue FY2026 Q2"), 500);
        assert!(result.relevant, "{result:?}");
        assert!(result.text.contains("$46.7 billion"));
    }

    #[test]
    fn ignores_navigation_and_hidden_content() {
        let page = "<nav><p>Revenue FY2026 Q2: $999 billion</p></nav><main><p hidden>Revenue FY2026 Q2: $888 billion</p><p>Revenue FY2026 Q2: $46.7 billion for NVIDIA.</p></main>";
        let result = html(page, Some("NVIDIA revenue FY2026 Q2"), 500);
        assert!(!result.text.contains("$999"));
        assert!(!result.text.contains("$888"));
        assert!(result.text.contains("$46.7"));
    }

    #[test]
    fn japanese_query_finds_japanese_evidence() {
        let page = "<title>NVIDIA 決算</title><main><p>売上高は前年同期を上回り、460億ドルとなりました。</p></main>";
        let result = html(page, Some("NVIDIAの売上高はいくら"), 500);
        assert!(result.relevant, "{result:?}");
    }

    #[test]
    fn plain_text_selects_relevant_line_instead_of_prefix() {
        let text = format!(
            "{}\nRevenue FY2026 Q2: $46.7 billion",
            "General information about NVIDIA.\n".repeat(100)
        );
        let result = plain(&text, Some("NVIDIA revenue FY2026 Q2"), 300);
        assert!(result.text.contains("$46.7 billion"));
    }

    #[test]
    fn body_is_considered_when_main_is_empty() {
        let page = "<title>NVIDIA results</title><main><div id='app'></div></main><section><p>NVIDIA revenue for FY2026 Q2 was $46.7 billion.</p></section>";
        let result = html(page, Some("NVIDIA revenue FY2026 Q2"), 500);
        assert!(result.relevant, "{result:?}");
    }

    #[test]
    fn javascript_shell_has_no_evidence() {
        let page = "<title>NVIDIA results</title><main><div id='app'></div></main><script>document.write('Revenue $999 billion')</script>";
        assert!(!html(page, Some("NVIDIA revenue FY2026 Q2"), 500).relevant);
    }

    #[test]
    fn value_at_end_of_long_paragraph_is_reachable() {
        let page = format!("<title>NVIDIA results</title><main><p>{} Revenue FY2026 Q2 was $46.7 billion.</p></main>", "Background information about the company. ".repeat(40));
        let result = html(&page, Some("NVIDIA revenue FY2026 Q2"), 500);
        assert!(result.relevant, "{result:?}");
        assert!(result.text.contains("$46.7 billion"));
    }

    #[test]
    fn fiscal_year_alone_is_not_a_financial_value() {
        let page = "<title>NVIDIA FY2026 Q2</title><main><p>Revenue for FY2026 Q2 has not been announced in this preview.</p></main>";
        assert!(!html(page, Some("NVIDIA revenue FY2026 Q2"), 500).relevant);
    }

    #[test]
    fn numeric_value_under_metric_heading_is_not_mislabelled() {
        let page = "<title>NVIDIA FY2026 Q2 results</title><main><h2>Revenue</h2><p>Operating expenses were $5 billion in the quarter.</p></main>";
        assert!(!html(page, Some("NVIDIA revenue FY2026 Q2"), 500).relevant);
    }
}
