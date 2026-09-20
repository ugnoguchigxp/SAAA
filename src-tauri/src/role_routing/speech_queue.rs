//! Deterministic single-owner queue for routing speech.
//!
//! Only persisted, final routing text is eligible for `Final`; acknowledgements are best-effort
//! and are discarded when a final answer is already ready.
use std::collections::{HashSet, VecDeque};

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
}

#[derive(Debug, Default)]
pub(crate) struct SpeechQueue {
    active: Option<SpeechItem>,
    queued: VecDeque<SpeechItem>,
    final_roots: HashSet<String>,
}

impl SpeechQueue {
    /// Returns false when the item would duplicate an already queued/playing final answer.
    pub(crate) fn enqueue(&mut self, item: SpeechItem) -> bool {
        if item.text.trim().is_empty() {
            return false;
        }
        if item.kind == SpeechKind::Final && !self.final_roots.insert(item.root_id.clone()) {
            return false;
        }
        if item.kind == SpeechKind::Final {
            self.queued.retain(|queued| {
                !(queued.root_id == item.root_id
                    && matches!(
                        queued.kind,
                        SpeechKind::Acknowledgement | SpeechKind::Progress
                    ))
            });
            self.queued.push_front(item);
        } else if !self.final_roots.contains(&item.root_id) {
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
        self.final_roots.remove(root_id);
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
