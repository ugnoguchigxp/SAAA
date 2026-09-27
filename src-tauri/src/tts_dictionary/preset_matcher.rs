use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

const SOURCE: &str = include_str!("../../data/tts-presets.tsv");

pub(crate) fn count() -> usize {
    SOURCE.lines().count()
}

pub(crate) fn lookup(written: &str) -> Option<String> {
    let first = written.chars().next()?;
    presets()
        .get(&first)?
        .iter()
        .find(|entry| entry.0 == written)
        .map(|entry| entry.1.clone())
}

pub(crate) fn search(
    query: &str,
    limit: usize,
    excluded: &HashSet<String>,
) -> Vec<(String, String)> {
    let query = query.trim();
    SOURCE
        .lines()
        .filter_map(|line| {
            let (written, spoken) = line.split_once('\t')?;
            (!excluded.contains(written)
                && (query.is_empty() || written.contains(query) || spoken.contains(query)))
            .then(|| (written.to_string(), spoken.to_string()))
        })
        .take(limit)
        .collect()
}

pub(crate) fn apply(text: &str, custom: &[(String, String)]) -> String {
    let mut sorted: Vec<_> = custom.iter().collect();
    sorted.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    while cursor < text.len() {
        let custom_match = sorted
            .iter()
            .find(|entry| text[cursor..].starts_with(entry.0.as_str()))
            .copied();
        let preset_match = text[cursor..]
            .chars()
            .next()
            .and_then(|character| presets().get(&character))
            .and_then(|group| {
                group
                    .iter()
                    .find(|entry| text[cursor..].starts_with(entry.0.as_str()))
            });
        if let Some(entry) = custom_match.or(preset_match) {
            output.push_str(&entry.1);
            cursor += entry.0.len();
        } else {
            let character = text[cursor..]
                .chars()
                .next()
                .expect("cursor is within text");
            output.push(character);
            cursor += character.len_utf8();
        }
    }
    output
}

/// Keep a trailing partial dictionary headword until the next streamed speech chunk arrives.
/// The returned prefix length is a UTF-8 byte boundary.
pub(crate) fn ready_prefix_len(text: &str, custom: &[(String, String)]) -> usize {
    let mut cursor = 0;
    while cursor < text.len() {
        let remaining = &text[cursor..];
        let custom_match = custom
            .iter()
            .filter(|entry| remaining.starts_with(entry.0.as_str()))
            .max_by_key(|entry| entry.0.len());
        if custom
            .iter()
            .any(|entry| entry.0.len() > remaining.len() && entry.0.starts_with(remaining))
        {
            return cursor;
        }
        if let Some(entry) = custom_match {
            cursor += entry.0.len();
            continue;
        }
        let character = remaining.chars().next().expect("cursor is within text");
        if let Some(group) = presets().get(&character) {
            if group
                .iter()
                .any(|entry| entry.0.len() > remaining.len() && entry.0.starts_with(remaining))
            {
                return cursor;
            }
            if let Some(entry) = group
                .iter()
                .find(|entry| remaining.starts_with(entry.0.as_str()))
            {
                cursor += entry.0.len();
                continue;
            }
        }
        cursor += character.len_utf8();
    }
    cursor
}

fn presets() -> &'static HashMap<char, Vec<(String, String)>> {
    static PRESETS: OnceLock<HashMap<char, Vec<(String, String)>>> = OnceLock::new();
    PRESETS.get_or_init(|| {
        let mut groups: HashMap<char, Vec<(String, String)>> = HashMap::new();
        for line in SOURCE.lines() {
            let (written, spoken) = line.split_once('\t').expect("preset TSV is valid");
            let first = written.chars().next().expect("preset headword is nonempty");
            groups
                .entry(first)
                .or_default()
                .push((written.to_string(), spoken.to_string()));
        }
        for group in groups.values_mut() {
            group.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
        }
        groups
    })
}
