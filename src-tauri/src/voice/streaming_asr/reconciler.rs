use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;
use zeroize::Zeroize;

#[derive(Debug, Clone)]
pub(crate) struct ComparableUnit {
    pub(crate) key: String,
    pub(crate) raw_end_byte: usize,
}
#[derive(Debug, Clone)]
pub(crate) struct ComparableTranscript {
    pub(crate) raw: String,
    pub(crate) units: Vec<ComparableUnit>,
}

impl Drop for ComparableTranscript {
    fn drop(&mut self) {
        self.raw.zeroize();
        for unit in &mut self.units {
            unit.key.zeroize();
        }
    }
}

pub(crate) fn comparable_transcript(raw: &str) -> ComparableTranscript {
    let raw: String = raw.nfc().collect();
    let mut units = Vec::new();
    let mut whitespace = false;
    for (offset, grapheme) in raw.grapheme_indices(true) {
        let end = offset + grapheme.len();
        if grapheme.chars().all(char::is_whitespace) {
            if !whitespace {
                units.push(ComparableUnit {
                    key: " ".to_string(),
                    raw_end_byte: end,
                });
                whitespace = true;
            }
        } else {
            units.push(ComparableUnit {
                key: grapheme.to_string(),
                raw_end_byte: end,
            });
            whitespace = false;
        }
    }
    ComparableTranscript { raw, units }
}
pub(crate) fn longest_common_prefix(left: &[ComparableUnit], right: &[ComparableUnit]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(a, b)| a.key == b.key)
        .count()
}
pub(crate) fn reconcile(
    previous: &[ComparableTranscript],
    stable_units: usize,
    latest: ComparableTranscript,
) -> Option<(usize, String, String)> {
    if latest.units.len() < stable_units {
        return None;
    }
    let mut candidate = stable_units;
    for hypothesis in previous.iter().rev().take(2) {
        candidate = candidate.max(longest_common_prefix(&latest.units, &hypothesis.units));
    }
    if candidate == latest.units.len() {
        candidate = candidate.saturating_sub(2).max(stable_units);
    }
    let boundary = candidate
        .checked_sub(1)
        .and_then(|index| latest.units.get(index))
        .map_or(0, |unit| unit.raw_end_byte);
    Some((
        candidate,
        latest.raw[..boundary].to_string(),
        latest.raw[boundary..].to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_nfc_and_whitespace() {
        let value = comparable_transcript("e\u{301}  x");
        assert_eq!(value.raw, "é  x");
        assert_eq!(value.units.len(), 3);
    }
    #[test]
    fn keeps_tail_unstable_until_a_later_hypothesis() {
        let first = comparable_transcript("クロードコ");
        let latest = comparable_transcript("クロードコード");
        let (_, stable, unstable) = reconcile(&[first], 0, latest).unwrap();
        assert_eq!(stable, "クロードコ");
        assert_eq!(unstable, "ード");
    }
    #[test]
    fn handles_latin_punctuation_and_conflicting_history_without_regression() {
        let latin = comparable_transcript("hello, world!");
        let latest = comparable_transcript("hello, world again!");
        let (_, stable, unstable) = reconcile(&[latin], 0, latest).unwrap();
        assert_eq!(format!("{stable}{unstable}"), "hello, world again!");

        let first = comparable_transcript("abc x");
        let second = comparable_transcript("abc y");
        let latest = comparable_transcript("abc z");
        let (units, stable, _) = reconcile(&[first, second], 0, latest).unwrap();
        assert_eq!(stable, "abc ");
        assert_eq!(units, 4);
    }
}
