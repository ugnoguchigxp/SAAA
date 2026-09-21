//! Deterministic single-owner queue for routing speech.
//!
//! Only persisted, final routing text is eligible for `Final`; acknowledgements are best-effort
//! and are discarded when a final answer is already ready. Every item is bound to the speaking
//! actor and root revision so stale or unattributed speech is never played.
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpeechKind {
    Acknowledgement,
    Progress,
    Final,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SpeechItem {
    pub(crate) id: String,
    pub(crate) root_id: String,
    pub(crate) kind: SpeechKind,
    pub(crate) text: String,
    /// Actor that authored the spoken text. Speech with no speaker is never queued.
    pub(crate) speaker: String,
    /// Root revision the text was produced under, so a superseded final is rejected.
    pub(crate) revision: u32,
}

impl SpeechItem {
    /// Binds a final answer to its persisted output id, speaker, and root revision.
    pub(crate) fn final_answer(
        answer_output_id: &str,
        root_id: &str,
        revision: u32,
        speaker: &str,
        text: &str,
    ) -> Self {
        Self {
            id: answer_output_id.to_string(),
            root_id: root_id.to_string(),
            kind: SpeechKind::Final,
            text: text.to_string(),
            speaker: speaker.to_string(),
            revision,
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SpeechQueue {
    active: Option<SpeechItem>,
    queued: VecDeque<SpeechItem>,
    /// Root -> revision of the final answer already accepted for that root.
    final_revisions: HashMap<String, u32>,
}

impl SpeechQueue {
    /// Returns false when the item would duplicate an already queued/playing final answer.
    pub(crate) fn enqueue(&mut self, item: SpeechItem) -> bool {
        if item.text.trim().is_empty() || item.speaker.trim().is_empty() {
            return false;
        }
        if item.kind == SpeechKind::Final {
            if let Some(seen) = self.final_revisions.get(&item.root_id) {
                // Already spoke (or reserved) this root, or the item is from an older revision.
                if item.revision <= *seen {
                    return false;
                }
            }
            self.final_revisions
                .insert(item.root_id.clone(), item.revision);
            self.queued.retain(|queued| {
                !(queued.root_id == item.root_id
                    && matches!(
                        queued.kind,
                        SpeechKind::Acknowledgement | SpeechKind::Progress
                    ))
            });
            self.queued.push_front(item);
        } else if !self.final_revisions.contains_key(&item.root_id) {
            self.queued.push_back(item);
        }
        true
    }

    /// Claims the only speech slot. Callers perform TTS outside this object and report `finish`.
    pub(crate) fn start_next(&mut self) -> Option<SpeechItem> {
        if self.active.is_none() {
            self.active = self.queued.pop_front();
        }
        self.active.clone()
    }

    pub(crate) fn finish(&mut self, id: &str) -> bool {
        if self.active.as_ref().is_some_and(|active| active.id == id) {
            self.active = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn cancel_root(&mut self, root_id: &str) -> Option<String> {
        self.queued.retain(|item| item.root_id != root_id);
        self.final_revisions.remove(root_id);
        self.active
            .as_ref()
            .filter(|active| active.root_id == root_id)
            .map(|active| active.id.clone())
    }

    #[cfg(test)]
    fn queued_len(&self) -> usize {
        self.queued.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, root: &str, kind: SpeechKind) -> SpeechItem {
        SpeechItem {
            id: id.into(),
            root_id: root.into(),
            kind,
            text: id.into(),
            speaker: "sol".into(),
            revision: 0,
        }
    }

    #[test]
    fn rr_13_final_before_ack_speaks_final_only() {
        let mut queue = SpeechQueue::default();
        assert!(queue.enqueue(item("ack", "root", SpeechKind::Acknowledgement)));
        assert!(queue.enqueue(item("final", "root", SpeechKind::Final)));
        assert_eq!(queue.start_next().expect("next").id, "final");
        assert_eq!(queue.queued_len(), 0);
        assert!(!queue.enqueue(item("duplicate", "root", SpeechKind::Final)));
    }

    #[test]
    fn rr_13_speech_is_bound_to_speaker_and_revision() {
        let mut queue = SpeechQueue::default();
        let mut unattributed = item("ack", "root", SpeechKind::Acknowledgement);
        unattributed.speaker = "  ".into();
        assert!(!queue.enqueue(unattributed));
        assert!(queue.enqueue(SpeechItem::final_answer(
            "answer-1", "root", 2, "sol", "done"
        )));
        // An older revision's final for the same root must be dropped.
        assert!(!queue.enqueue(SpeechItem::final_answer(
            "answer-0", "root", 1, "sol", "older"
        )));
        assert_eq!(queue.start_next().expect("final").id, "answer-1");
    }

    #[test]
    fn rr_13_ack_stop_timeout_leaves_one_active_speech() {
        let mut queue = SpeechQueue::default();
        queue.enqueue(item("ack", "root", SpeechKind::Acknowledgement));
        queue.enqueue(item("progress", "other", SpeechKind::Progress));
        assert_eq!(queue.start_next().expect("active").id, "ack");
        assert_eq!(queue.start_next().expect("same active").id, "ack");
        assert_eq!(queue.cancel_root("root"), Some("ack".into()));
        assert!(queue.finish("ack"));
        assert_eq!(queue.start_next().expect("next root").id, "progress");
    }
}
