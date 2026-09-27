//! Keeps control tags out of audio even when an SSE frame splits the tag.

#[derive(Default)]
pub(crate) struct SpeechProjection {
    pending: String,
    projected: String,
    seen: String,
    blocked: bool,
    length: usize,
}

impl SpeechProjection {
    pub(crate) fn blocked(&self) -> bool {
        self.blocked || !self.pending.is_empty()
    }

    pub(crate) fn rejected(&self) -> bool {
        self.blocked
    }

    pub(crate) fn complete(&self, answer: &str) -> bool {
        !self.blocked() && self.projected == answer
    }

    pub(crate) fn append(&mut self, text: &str) -> String {
        if self.blocked {
            return String::new();
        }
        self.length += text.len();
        if self.length > 8192 {
            self.blocked = true;
            return String::new();
        }
        self.seen.push_str(text);
        self.pending.push_str(text);
        let mut safe = String::new();
        const MARKERS: [&str; 3] = ["<think>", "</think>", "<|"];
        while !self.pending.is_empty() {
            if self.pending.starts_with('<') {
                if MARKERS
                    .iter()
                    .any(|marker| self.pending.starts_with(marker))
                {
                    self.blocked = true;
                    self.pending.clear();
                    break;
                }
                if MARKERS
                    .iter()
                    .any(|marker| marker.starts_with(&self.pending))
                {
                    break;
                }
                safe.push('<');
                self.pending.remove(0);
            } else if let Some(index) = self.pending.find('<') {
                safe.push_str(&self.pending[..index]);
                self.pending.drain(..index);
            } else {
                safe.push_str(&self.pending);
                self.pending.clear();
            }
        }
        self.projected.push_str(&safe);
        safe
    }

    pub(crate) fn finish(&mut self, answer: &str) -> Option<String> {
        if self.blocked || !answer.starts_with(&self.seen) {
            return None;
        }
        let suffix = answer[self.seen.len()..].to_string();
        let remainder = self.append(&suffix);
        if self.complete(answer) {
            Some(remainder)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SpeechProjection;

    #[test]
    fn completes_missing_deltas_from_final_answer() {
        let mut projection = SpeechProjection::default();
        assert_eq!(projection.append("こん"), "こん");
        assert_eq!(projection.finish("こんにちは"), Some("にちは".into()));
        assert_eq!(
            SpeechProjection::default().finish("回答"),
            Some("回答".into())
        );
    }

    #[test]
    fn rejects_mismatched_or_control_deltas() {
        let mut projection = SpeechProjection::default();
        projection.append("別の文字列");
        assert_eq!(projection.finish("回答"), None);
        let mut projection = SpeechProjection::default();
        projection.append("<thi");
        projection.append("nk>");
        assert!(projection.rejected());
        assert_eq!(projection.finish("回答"), None);
        let mut truncated = SpeechProjection::default();
        truncated.append("回答<thi");
        assert_eq!(truncated.finish("回答<thi"), None);

        let mut oversized = SpeechProjection::default();
        oversized.append(&"あ".repeat(2731));
        assert!(oversized.rejected());
    }
}
