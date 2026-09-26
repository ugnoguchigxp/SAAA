//! Observation-only ASR update scheduling. The caller owns the monotonic clock and
//! the single Qwen slot; this module never starts a provider, persists work, or speaks.
use std::collections::{HashMap, VecDeque};

const PERIOD_MS: u64 = 1_500;
const MAX_PENDING: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Update {
    pub session_id: String,
    pub conversation_id: String,
    pub utterance_id: String,
    pub asr_revision: u64,
    pub text: String,
    pub final_result: bool,
    pub stop_candidate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Intake {
    Accepted,
    Ignored,
    Full,
    StaleSession,
}

#[derive(Default)]
pub(crate) struct Dispatcher {
    session: Option<(String, String)>,
    latest: HashMap<String, Update>,
    utterance_order: Vec<String>,
    sent: HashMap<String, (u64, bool)>,
    pending: VecDeque<Update>,
    in_flight: Option<Update>,
    next_tick_ms: Option<u64>,
}

impl Dispatcher {
    pub(crate) fn start_session(
        &mut self,
        session_id: String,
        conversation_id: String,
        now_ms: u64,
    ) {
        self.session = Some((session_id, conversation_id));
        self.latest.clear();
        self.utterance_order.clear();
        self.sent.clear();
        self.pending.clear();
        self.in_flight = None;
        self.next_tick_ms = Some(now_ms.saturating_add(PERIOD_MS));
    }

    pub(crate) fn stop_session(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn update(&mut self, update: Update, now_ms: u64) -> Intake {
        if self.session.as_ref()
            != Some(&(update.session_id.clone(), update.conversation_id.clone()))
        {
            return Intake::StaleSession;
        }
        if update.text.trim().is_empty() {
            return Intake::Ignored;
        }
        if self
            .sent
            .get(&update.utterance_id)
            .is_some_and(|(revision, final_result)| {
                *revision > update.asr_revision
                    || (*revision == update.asr_revision && (*final_result || !update.final_result))
            })
        {
            return Intake::Ignored;
        }
        if let Some(previous) = self.latest.get(&update.utterance_id) {
            if update.asr_revision < previous.asr_revision
                || (update.asr_revision == previous.asr_revision
                    && (!update.final_result || previous.final_result))
            {
                return Intake::Ignored;
            }
        }
        if self.pending.len() >= MAX_PENDING
            && !self
                .pending
                .iter()
                .any(|item| item.utterance_id == update.utterance_id && !item.final_result)
        {
            return Intake::Full;
        }
        if self.latest.len() >= MAX_PENDING && !self.latest.contains_key(&update.utterance_id) {
            return Intake::Full;
        }
        let immediate = update.final_result || update.stop_candidate;
        if !self.latest.contains_key(&update.utterance_id) {
            self.utterance_order.push(update.utterance_id.clone());
        }
        self.latest
            .insert(update.utterance_id.clone(), update.clone());
        if immediate {
            self.enqueue(update);
        } else {
            self.tick(now_ms);
        }
        Intake::Accepted
    }

    pub(crate) fn boundary(&mut self, utterance_id: &str) {
        if let Some(update) = self.latest.get(utterance_id).cloned() {
            self.enqueue(update);
        }
    }

    pub(crate) fn boundary_latest(&mut self) {
        if let Some(id) = self.utterance_order.last().cloned() {
            self.boundary(&id);
        }
    }

    pub(crate) fn discard(&mut self, utterance_id: &str) {
        self.latest.remove(utterance_id);
        self.utterance_order.retain(|id| id != utterance_id);
        self.pending
            .retain(|update| update.utterance_id != utterance_id);
    }

    pub(crate) fn tick(&mut self, now_ms: u64) {
        let Some(next) = self.next_tick_ms else {
            return;
        };
        if now_ms < next {
            return;
        }
        self.next_tick_ms = Some(now_ms.saturating_add(PERIOD_MS));
        let updates: Vec<_> = self
            .utterance_order
            .iter()
            .filter_map(|id| self.latest.get(id))
            .filter(|update| !update.final_result)
            .cloned()
            .collect();
        for update in updates {
            self.enqueue(update);
        }
    }

    fn enqueue(&mut self, update: Update) {
        if self
            .sent
            .get(&update.utterance_id)
            .is_some_and(|(revision, final_result)| {
                *revision > update.asr_revision
                    || (*revision == update.asr_revision && (*final_result || !update.final_result))
            })
            || self.in_flight.as_ref().is_some_and(|item| {
                item.utterance_id == update.utterance_id
                    && (item.asr_revision > update.asr_revision
                        || (item.asr_revision == update.asr_revision
                            && (item.final_result || !update.final_result)))
            })
        {
            return;
        }
        if let Some(position) = self
            .pending
            .iter()
            .position(|item| item.utterance_id == update.utterance_id && !item.final_result)
        {
            self.pending.remove(position);
        }
        if update.stop_candidate {
            self.pending.push_front(update);
        } else {
            self.pending.push_back(update);
        }
    }

    pub(crate) fn take_ready(&mut self) -> Option<Update> {
        if self.in_flight.is_some() {
            return None;
        }
        let update = self.pending.pop_front()?;
        self.sent.insert(
            update.utterance_id.clone(),
            (update.asr_revision, update.final_result),
        );
        self.in_flight = Some(update.clone());
        Some(update)
    }

    pub(crate) fn finish(&mut self) {
        if let Some(update) = self.in_flight.take() {
            if update.final_result
                && self.latest.get(&update.utterance_id).is_some_and(|latest| {
                    latest.final_result && latest.asr_revision == update.asr_revision
                })
            {
                self.latest.remove(&update.utterance_id);
                self.utterance_order.retain(|id| id != &update.utterance_id);
            }
        }
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub(crate) fn next_tick_ms(&self) -> Option<u64> {
        self.next_tick_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(id: &str, revision: u64, final_result: bool) -> Update {
        Update {
            session_id: "s".into(),
            conversation_id: "c".into(),
            utterance_id: id.into(),
            asr_revision: revision,
            text: "hello".into(),
            final_result,
            stop_candidate: false,
        }
    }
    fn dispatcher() -> Dispatcher {
        let mut d = Dispatcher::default();
        d.start_session("s".into(), "c".into(), 0);
        d
    }

    #[test]
    fn periodic_boundary_and_final_are_sent_once_per_revision() {
        let mut d = dispatcher();
        assert_eq!(d.update(update("a", 1, false), 1_499), Intake::Accepted);
        assert!(d.take_ready().is_none());
        d.tick(1_500);
        d.boundary("a");
        assert_eq!(d.take_ready().unwrap().asr_revision, 1);
        d.finish();
        assert!(d.take_ready().is_none());
        assert_eq!(d.update(update("a", 2, true), 1_501), Intake::Accepted);
        assert_eq!(d.take_ready().unwrap().asr_revision, 2);
    }

    #[test]
    fn busy_slot_coalesces_partial_but_keeps_other_utterances_and_final() {
        let mut d = dispatcher();
        d.update(update("a", 1, false), 0);
        d.boundary("a");
        d.take_ready();
        for revision in 2..=4 {
            d.update(update("a", revision, false), 1_500);
        }
        d.update(update("b", 1, true), 1_500);
        d.update(update("a", 5, true), 1_500);
        assert_eq!(d.pending_count(), 2);
        d.finish();
        assert_eq!(d.take_ready().unwrap().utterance_id, "b");
        d.finish();
        assert_eq!(d.take_ready().unwrap().asr_revision, 5);
    }

    #[test]
    fn stale_session_and_duplicate_are_ignored() {
        let mut d = dispatcher();
        assert_eq!(d.update(update("a", 1, false), 0), Intake::Accepted);
        assert_eq!(d.update(update("a", 1, false), 0), Intake::Ignored);
        assert_eq!(d.update(update("a", 1, true), 0), Intake::Accepted);
        assert!(d.take_ready().unwrap().final_result);
        d.start_session("new".into(), "other".into(), 5);
        assert_eq!(d.update(update("a", 2, true), 5), Intake::StaleSession);
        assert_eq!(d.pending_count(), 0);
    }

    #[test]
    fn full_queue_does_not_evict_another_utterance() {
        let mut d = dispatcher();
        for index in 0..MAX_PENDING {
            d.update(update(&index.to_string(), 1, true), 0);
        }
        assert_eq!(d.update(update("overflow", 1, true), 0), Intake::Full);
        assert_eq!(d.pending_count(), MAX_PENDING);
    }

    #[test]
    fn stop_candidate_precedes_waiting_work_without_interrupting_in_flight() {
        let mut d = dispatcher();
        d.update(update("running", 1, true), 0);
        d.take_ready();
        d.update(update("normal", 1, true), 1);
        let mut stop = update("stop", 1, false);
        stop.stop_candidate = true;
        d.update(stop, 2);
        assert!(d.take_ready().is_none());
        d.finish();
        assert_eq!(d.take_ready().unwrap().utterance_id, "stop");
        d.finish();
        assert_eq!(d.take_ready().unwrap().utterance_id, "normal");
    }

    #[test]
    fn periodic_updates_keep_first_seen_utterance_order() {
        let mut d = dispatcher();
        d.update(update("first", 1, false), 1);
        d.update(update("second", 1, false), 2);
        d.tick(1_500);
        assert_eq!(d.take_ready().unwrap().utterance_id, "first");
        d.finish();
        assert_eq!(d.take_ready().unwrap().utterance_id, "second");
    }

    #[test]
    fn finalization_without_text_change_is_still_delivered() {
        let mut d = dispatcher();
        d.update(update("a", 1, false), 0);
        d.boundary("a");
        assert!(!d.take_ready().unwrap().final_result);
        d.finish();
        assert_eq!(d.update(update("a", 1, true), 1), Intake::Accepted);
        assert!(d.take_ready().unwrap().final_result);
    }

    #[test]
    fn slow_consumer_keeps_latest_partial_and_late_correction() {
        let mut d = dispatcher();
        d.update(update("a", 1, false), 0);
        d.tick(1_500);
        assert_eq!(d.take_ready().unwrap().asr_revision, 1);
        d.update(update("a", 2, false), 1_600);
        d.tick(3_000);
        d.update(update("a", 3, true), 3_001);
        assert_eq!(d.pending_count(), 1);
        d.finish();
        assert_eq!(d.take_ready().unwrap().asr_revision, 3);
    }

    #[test]
    fn active_limit_releases_after_final_but_replay_stays_ignored() {
        let mut d = dispatcher();
        for index in 0..MAX_PENDING {
            d.update(update(&index.to_string(), 1, false), 0);
        }
        assert_eq!(d.update(update("overflow", 1, false), 0), Intake::Full);
        d.update(update("0", 2, true), 1);
        assert_eq!(d.take_ready().unwrap().utterance_id, "0");
        d.finish();
        assert_eq!(d.update(update("0", 2, true), 2), Intake::Ignored);
        assert_eq!(d.update(update("new", 1, false), 2), Intake::Accepted);
    }

    #[test]
    fn boundary_targets_latest_active_utterance_and_discard_removes_it() {
        let mut d = dispatcher();
        d.update(update("a", 1, false), 0);
        d.update(update("b", 1, false), 1);
        d.boundary_latest();
        assert_eq!(d.take_ready().unwrap().utterance_id, "b");
        d.finish();
        d.discard("b");
        d.tick(1_500);
        assert_eq!(d.take_ready().unwrap().utterance_id, "a");
        d.finish();
        assert!(d.take_ready().is_none());
    }
}
