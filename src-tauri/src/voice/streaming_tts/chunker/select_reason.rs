use super::*;
pub(crate) const FIRST_MIN: usize = 10;
pub(crate) const STEADY_MIN: usize = 24;
pub(crate) const TARGET: usize = 120;
pub(crate) const HARD_MAX: usize = 240;
pub(crate) const MAX_SOURCE_CHARS: usize = 64_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectReason {
    Append,
    Idle,
    Completion,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpeechChunk {
    pub(crate) raw: String,
    pub(crate) spoken: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccumulatorError {
    SourceTooLarge,
    SyncLost,
}
#[derive(Debug, Default)]
pub(crate) struct SentenceAccumulator {
    pub(super) source: String,
    pub(super) source_chars: usize,
    pub(super) consumed_byte: usize,
    pub(super) first_chunk_claimed: bool,
    pub(super) input_closed: bool,
    pub(super) idle_generation: u64,
    pub(super) protected_tail: Option<ProtectedTail>,
}
#[derive(Debug)]
pub(super) enum ProtectedTail {
    Fence {
        start: usize,
        marker: &'static str,
        search_from: usize,
    },
    InlineCode {
        start: usize,
        marker: &'static str,
        search_from: usize,
    },
    Url {
        start: usize,
        search_from: usize,
    },
}
impl SentenceAccumulator {
    pub(crate) fn append(&mut self, delta: &str) -> Result<(), AccumulatorError> {
        if self.input_closed || delta.is_empty() {
            return Ok(());
        }
        let delta_chars = delta.chars().count();
        if self.source_chars.saturating_add(delta_chars) > MAX_SOURCE_CHARS {
            return Err(AccumulatorError::SourceTooLarge);
        }
        self.source.push_str(delta);
        self.source_chars += delta_chars;
        self.idle_generation = self.idle_generation.wrapping_add(1);
        self.update_protected_tail();
        Ok(())
    }

    pub(crate) fn finish(&mut self, final_content: &str) -> Result<(), AccumulatorError> {
        if self.input_closed {
            return Ok(());
        }
        if final_content == self.source {
            self.input_closed = true;
            return Ok(());
        }
        let Some(suffix) = final_content.strip_prefix(&self.source) else {
            return Err(AccumulatorError::SyncLost);
        };
        self.append(suffix)?;
        self.input_closed = true;
        Ok(())
    }

    pub(crate) fn idle_generation(&self) -> u64 {
        self.idle_generation
    }

    pub(crate) fn next_chunk(&mut self, reason: SelectReason) -> Option<SpeechChunk> {
        loop {
            let end = self.select_end(reason)?;
            let raw = self.source[self.consumed_byte..end].to_string();
            self.consumed_byte = end;
            if self
                .protected_tail
                .as_ref()
                .is_some_and(|tail| tail.start() < self.consumed_byte)
            {
                self.protected_tail = None;
            }
            let spoken = text_for_speech(&raw);
            if spoken.is_empty() {
                continue;
            }
            self.first_chunk_claimed = true;
            return Some(SpeechChunk { raw, spoken });
        }
    }

    #[cfg(test)]
    pub(crate) fn is_drained(&self) -> bool {
        self.input_closed && self.consumed_byte == self.source.len()
    }

    fn select_end(&self, reason: SelectReason) -> Option<usize> {
        if self.consumed_byte >= self.source.len() {
            return None;
        }
        let raw = &self.source[self.consumed_byte..];
        if !self.input_closed
            && self
                .protected_tail
                .as_ref()
                .is_some_and(|tail| tail.start() == self.consumed_byte)
        {
            return None;
        }
        let boundaries = scan_boundaries(raw, self.input_closed);
        let minimum = if self.first_chunk_claimed {
            STEADY_MIN
        } else {
            FIRST_MIN
        };

        if let Some(boundary) = boundaries.strong.iter().copied().find(|end| {
            let length = speakable_len(&raw[..*end]);
            (minimum..=HARD_MAX).contains(&length)
        }) {
            return Some(self.consumed_byte + boundary);
        }

        if reason == SelectReason::Idle {
            if let Some(boundary) = boundaries
                .weak
                .iter()
                .copied()
                .rfind(|end| speakable_len(&raw[..*end]) >= minimum)
            {
                return Some(self.consumed_byte + boundary);
            }
        }

        let current_len = speakable_len(raw);
        if current_len >= TARGET {
            if let Some(boundary) = boundaries.weak.iter().copied().rfind(|end| {
                let length = speakable_len(&raw[..*end]);
                (minimum..=TARGET).contains(&length)
            }) {
                return Some(self.consumed_byte + boundary);
            }
        }

        if current_len >= HARD_MAX {
            if let Some(boundary) = boundaries.weak.iter().copied().rfind(|end| {
                let length = speakable_len(&raw[..*end]);
                (minimum..=HARD_MAX).contains(&length)
            }) {
                return Some(self.consumed_byte + boundary);
            }
            if let Some(boundary) = boundaries.safe.iter().copied().rfind(|end| {
                let length = speakable_len(&raw[..*end]);
                (minimum..=HARD_MAX).contains(&length)
            }) {
                return Some(self.consumed_byte + boundary);
            }
        }

        if reason == SelectReason::Completion && self.input_closed {
            return Some(self.source.len());
        }
        None
    }

    fn update_protected_tail(&mut self) {
        let mut clear = false;
        match self.protected_tail.as_mut() {
            Some(ProtectedTail::Fence {
                marker,
                search_from,
                ..
            }) => {
                let pattern = format!("\n{marker}");
                if self.source[*search_from..].contains(&pattern) {
                    clear = true;
                } else {
                    *search_from = self.source.len().saturating_sub(pattern.len());
                }
            }
            Some(ProtectedTail::InlineCode {
                marker,
                search_from,
                ..
            }) => {
                if self.source[*search_from..].contains(*marker) {
                    clear = true;
                } else {
                    *search_from = self.source.len().saturating_sub(marker.len());
                }
            }
            Some(ProtectedTail::Url { search_from, .. }) => {
                if self.source[*search_from..].chars().any(|character| {
                    character.is_whitespace() || matches!(character, '<' | '>' | '"')
                }) {
                    clear = true;
                } else {
                    *search_from = self.source.len();
                }
            }
            None => {}
        }
        if clear {
            self.protected_tail = None;
        }
        if self.protected_tail.is_some() || self.consumed_byte >= self.source.len() {
            return;
        }
        let raw = &self.source[self.consumed_byte..];
        for marker in ["```", "~~~"] {
            if raw.starts_with(marker) && !raw[marker.len()..].contains(&format!("\n{marker}")) {
                self.protected_tail = Some(ProtectedTail::Fence {
                    start: self.consumed_byte,
                    marker,
                    search_from: self.consumed_byte + marker.len(),
                });
                return;
            }
        }
        let inline_marker = if raw.starts_with("``") {
            Some("``")
        } else if raw.starts_with('`') {
            Some("`")
        } else {
            None
        };
        if let Some(marker) = inline_marker {
            if !raw[marker.len()..].contains(marker) {
                self.protected_tail = Some(ProtectedTail::InlineCode {
                    start: self.consumed_byte,
                    marker,
                    search_from: self.consumed_byte + marker.len(),
                });
                return;
            }
        }
        if starts_with_url(raw)
            && !raw
                .chars()
                .any(|character| character.is_whitespace() || matches!(character, '<' | '>' | '"'))
        {
            self.protected_tail = Some(ProtectedTail::Url {
                start: self.consumed_byte,
                search_from: self.source.len(),
            });
        }
    }
}
impl ProtectedTail {
    fn start(&self) -> usize {
        match self {
            Self::Fence { start, .. }
            | Self::InlineCode { start, .. }
            | Self::Url { start, .. } => *start,
        }
    }
}
#[derive(Debug, Default)]
pub(super) struct Boundaries {
    pub(super) strong: Vec<usize>,
    pub(super) weak: Vec<usize>,
    pub(super) safe: Vec<usize>,
}
pub(super) fn speakable_len(input: &str) -> usize {
    text_for_speech(input).graphemes(true).count()
}
pub(super) fn scan_boundaries(input: &str, input_closed: bool) -> Boundaries {
    let mut boundaries = Boundaries::default();
    let mut cursor = 0;
    let mut delimiters = Vec::new();
    let mut deferred_strong = false;

    while cursor < input.len() {
        if let Some(end) = fenced_code_end(input, cursor) {
            cursor = end;
            continue;
        }
        if let Some(end) = inline_code_end(input, cursor) {
            cursor = end;
            continue;
        }
        if let Some(end) = markdown_link_end(input, cursor) {
            cursor = end;
            continue;
        }
        if let Some(end) = url_end(input, cursor) {
            cursor = end;
            continue;
        }
        if let Some(end) = html_tag_end(input, cursor) {
            cursor = end;
            continue;
        }

        let Some((_, grapheme)) = input[cursor..].grapheme_indices(true).next() else {
            break;
        };
        let end = cursor + grapheme.len();
        let depth_before = delimiters.len();

        if let Some(delimiter) = opening_delimiter(grapheme) {
            if delimiters.len() < 64 {
                delimiters.push(delimiter);
            }
            cursor = end;
            continue;
        }
        if is_closing_delimiter(grapheme, delimiters.last().copied()) {
            delimiters.pop();
            if delimiters.is_empty() && deferred_strong {
                boundaries.strong.push(end);
                deferred_strong = false;
            }
            cursor = end;
            continue;
        }

        let strong = is_japanese_sentence_end(grapheme)
            || is_newline(grapheme)
            || is_ascii_sentence_end(input, cursor, end, input_closed);
        if strong {
            if depth_before == 0 {
                boundaries.strong.push(end);
            } else {
                deferred_strong = true;
            }
        } else if depth_before == 0 && is_weak_boundary(input, cursor, end, grapheme) {
            boundaries.weak.push(end);
        }

        if depth_before == 0 && is_safe_boundary(input, end) {
            boundaries.safe.push(end);
        }
        cursor = end;
    }
    boundaries
}
pub(super) fn fenced_code_end(input: &str, cursor: usize) -> Option<usize> {
    let marker = if input[cursor..].starts_with("```") {
        "```"
    } else if input[cursor..].starts_with("~~~") {
        "~~~"
    } else {
        return None;
    };
    let line_start = cursor == 0 || input[..cursor].ends_with('\n');
    if !line_start {
        return None;
    }
    let after_open = cursor + marker.len();
    let close_from = input[after_open..]
        .find(&format!("\n{marker}"))
        .map(|offset| after_open + offset + 1 + marker.len());
    close_from.or(Some(input.len()))
}
pub(super) fn inline_code_end(input: &str, cursor: usize) -> Option<usize> {
    if !input[cursor..].starts_with('`') || input[cursor..].starts_with("```") {
        return None;
    }
    let marker_len = if input[cursor..].starts_with("``") {
        2
    } else {
        1
    };
    let marker = &input[cursor..cursor + marker_len];
    input[cursor + marker_len..]
        .find(marker)
        .map(|offset| cursor + marker_len + offset + marker_len)
        .or(Some(input.len()))
}
pub(super) fn markdown_link_end(input: &str, cursor: usize) -> Option<usize> {
    let start = if input[cursor..].starts_with("![") {
        cursor + 1
    } else if input[cursor..].starts_with('[') {
        cursor
    } else {
        return None;
    };
    let label_end = input[start + 1..].find("](")? + start + 1;
    let destination_start = label_end + 2;
    input[destination_start..]
        .find(')')
        .map(|offset| destination_start + offset + 1)
        .or(Some(input.len()))
}
pub(super) fn url_end(input: &str, cursor: usize) -> Option<usize> {
    let suffix = &input[cursor..];
    if !starts_with_url(suffix) {
        return None;
    }
    let mut end = cursor;
    for (offset, character) in suffix.char_indices() {
        if character.is_whitespace() || matches!(character, '<' | '>' | '"') {
            break;
        }
        end = cursor + offset + character.len_utf8();
    }
    while end > cursor {
        let character = input[..end].chars().next_back().expect("end is valid");
        if !matches!(character, '。' | '！' | '？' | '.' | ',' | '!' | '?') {
            break;
        }
        end -= character.len_utf8();
    }
    Some(end.max(cursor + 1))
}
pub(super) fn starts_with_url(input: &str) -> bool {
    ["http://", "https://", "www."].iter().any(|prefix| {
        input
            .get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
    })
}
pub(super) fn html_tag_end(input: &str, cursor: usize) -> Option<usize> {
    if !input[cursor..].starts_with('<') {
        return None;
    }
    input[cursor..]
        .find('>')
        .map(|offset| cursor + offset + 1)
        .or(Some(input.len()))
}
pub(super) fn opening_delimiter(grapheme: &str) -> Option<char> {
    match grapheme {
        "(" => Some('('),
        "[" => Some('['),
        "{" => Some('{'),
        "「" => Some('「'),
        "『" => Some('『'),
        "“" => Some('“'),
        "‘" => Some('‘'),
        "\"" => Some('"'),
        _ => None,
    }
}
pub(super) fn is_closing_delimiter(grapheme: &str, open: Option<char>) -> bool {
    matches!(
        (open, grapheme),
        (Some('('), ")")
            | (Some('['), "]")
            | (Some('{'), "}")
            | (Some('「'), "」")
            | (Some('『'), "』")
            | (Some('“'), "”")
            | (Some('‘'), "’")
            | (Some('"'), "\"")
    )
}
pub(super) fn is_japanese_sentence_end(grapheme: &str) -> bool {
    matches!(grapheme, "。" | "！" | "？")
}
pub(super) fn is_newline(grapheme: &str) -> bool {
    matches!(grapheme, "\n" | "\r")
}
pub(super) fn is_ascii_sentence_end(
    input: &str,
    cursor: usize,
    end: usize,
    input_closed: bool,
) -> bool {
    let Some(character) = input[cursor..end].chars().next() else {
        return false;
    };
    if !matches!(character, '.' | '!' | '?') {
        return false;
    }
    if character == '.' {
        let before_is_digit = input[..cursor]
            .chars()
            .next_back()
            .is_some_and(|value| value.is_ascii_digit());
        let after_is_digit = input[end..]
            .chars()
            .next()
            .is_some_and(|value| value.is_ascii_digit());
        if before_is_digit && after_is_digit {
            return false;
        }
    }
    let tail = &input[end..];
    if tail.is_empty() {
        return input_closed;
    }
    let mut chars = tail.chars();
    while matches!(
        chars.clone().next(),
        Some(')' | ']' | '}' | '」' | '』' | '”' | '’' | '"')
    ) {
        chars.next();
    }
    match chars.next() {
        None => input_closed,
        Some(value) => value.is_whitespace(),
    }
}
