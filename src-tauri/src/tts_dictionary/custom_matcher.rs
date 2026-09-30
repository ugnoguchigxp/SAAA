use std::collections::HashMap;

/// Immutable snapshot used for one speech turn.
pub(crate) struct CompiledDictionary {
    groups: HashMap<char, Vec<(String, String)>>,
}

impl CompiledDictionary {
    pub(crate) fn new(entries: Vec<(String, String)>) -> Self {
        let mut groups: HashMap<char, Vec<(String, String)>> = HashMap::new();
        for entry in entries {
            if let Some(first) = entry.0.chars().next() {
                groups.entry(first).or_default().push(entry);
            }
        }
        for group in groups.values_mut() {
            group.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
        }
        Self { groups }
    }

    pub(crate) fn apply(&self, text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        let mut cursor = 0;
        while cursor < text.len() {
            let remaining = &text[cursor..];
            let first = remaining.chars().next().expect("cursor is within text");
            let matched = self.groups.get(&first).and_then(|group| {
                group
                    .iter()
                    .find(|entry| remaining.starts_with(entry.0.as_str()))
            });
            if let Some((written, spoken)) = matched {
                output.push_str(spoken);
                cursor += written.len();
            } else {
                output.push(first);
                cursor += first.len_utf8();
            }
        }
        output
    }

    /// Keep a trailing partial headword until the next streamed speech chunk arrives.
    pub(crate) fn ready_prefix_len(&self, text: &str) -> usize {
        let mut cursor = 0;
        while cursor < text.len() {
            let remaining = &text[cursor..];
            let first = remaining.chars().next().expect("cursor is within text");
            if let Some(group) = self.groups.get(&first) {
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
            cursor += first.len_utf8();
        }
        cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_custom_entries_change_speech() {
        let empty = CompiledDictionary::new(vec![]);
        assert_eq!(empty.apply("今日は銀行へ"), "今日は銀行へ");
        let custom = CompiledDictionary::new(vec![("今日".into(), "きょう".into())]);
        assert_eq!(custom.apply("今日"), "きょう");
    }

    #[test]
    fn waits_for_partial_headwords_and_uses_longest_match() {
        let custom = CompiledDictionary::new(vec![
            ("Open".into(), "オープン".into()),
            ("OpenAI".into(), "オープンエーアイ".into()),
        ]);
        assert_eq!(custom.ready_prefix_len("OpenA"), 0);
        assert_eq!(custom.apply("OpenAI"), "オープンエーアイ");
        assert_eq!(custom.ready_prefix_len("OpenAIです"), "OpenAIです".len());
    }

    #[test]
    fn handles_thousands_of_entries_without_changing_unrelated_text() {
        let entries = (0..5_000)
            .map(|number| (format!("単語{number:04}"), format!("タンゴ{number:04}")))
            .collect();
        let custom = CompiledDictionary::new(entries);
        assert_eq!(custom.apply("今日は単語4999です"), "今日はタンゴ4999です");
        assert_eq!(custom.ready_prefix_len("今日は単語499"), "今日は".len());
    }
}
