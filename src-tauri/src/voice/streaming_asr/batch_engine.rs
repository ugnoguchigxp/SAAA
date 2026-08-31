use super::reconciler::{comparable_transcript, reconcile, ComparableTranscript};
use std::collections::VecDeque;

pub(crate) const BATCH_MIN_SAMPLES: u64 = 28_800; // 1,800ms @ 16k
pub(crate) const BATCH_HOP_SAMPLES: u64 = 9_600; // 600ms @ 16k
const HYPOTHESIS_CAPACITY: usize = 3;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DecodeKind {
    Partial,
    Final,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodeRequest {
    pub(crate) kind: DecodeKind,
    pub(crate) start_sample: u64,
    pub(crate) end_sample: u64,
    pub(crate) generation: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Projection {
    pub(crate) stable: String,
    pub(crate) unstable: String,
}
pub(crate) struct BatchEngine {
    generation: u64,
    in_flight: Option<DecodeRequest>,
    pending_end: Option<u64>,
    next_partial_at: u64,
    hypotheses: VecDeque<ComparableTranscript>,
    stable_units: usize,
}
impl Default for BatchEngine {
    fn default() -> Self {
        Self {
            generation: 0,
            in_flight: None,
            pending_end: None,
            next_partial_at: BATCH_MIN_SAMPLES,
            hypotheses: VecDeque::new(),
            stable_units: 0,
        }
    }
}
impl BatchEngine {
    pub(crate) fn on_audio(&mut self, utterance_end: u64) -> Option<DecodeRequest> {
        if utterance_end < self.next_partial_at {
            return None;
        }
        while self.next_partial_at <= utterance_end {
            self.next_partial_at += BATCH_HOP_SAMPLES;
        }
        if self.in_flight.is_some() {
            self.pending_end = Some(utterance_end);
            return None;
        }
        self.start(DecodeKind::Partial, utterance_end)
    }
    pub(crate) fn on_partial_complete(
        &mut self,
        end_sample: u64,
        text: &str,
    ) -> (Option<Projection>, Option<DecodeRequest>) {
        self.in_flight = None;
        let projection = self.apply_hypothesis(text);
        let next = self.pending_end.take().and_then(|pending| {
            (pending > end_sample)
                .then(|| self.start(DecodeKind::Partial, pending))
                .flatten()
        });
        (projection, next)
    }
    pub(crate) fn on_partial_failed(&mut self, end_sample: u64) -> Option<DecodeRequest> {
        self.in_flight = None;
        self.pending_end.take().and_then(|pending| {
            (pending > end_sample)
                .then(|| self.start(DecodeKind::Partial, pending))
                .flatten()
        })
    }
    pub(crate) fn commit(&mut self, utterance_end: u64) -> DecodeRequest {
        self.generation += 1;
        self.in_flight = None;
        self.pending_end = None;
        self.start(DecodeKind::Final, utterance_end)
            .expect("final request always starts")
    }
    fn start(&mut self, kind: DecodeKind, end_sample: u64) -> Option<DecodeRequest> {
        if end_sample == 0 {
            return None;
        }
        let request = DecodeRequest {
            kind,
            start_sample: 0,
            end_sample,
            generation: self.generation,
        };
        self.in_flight = Some(request.clone());
        Some(request)
    }
    fn apply_hypothesis(&mut self, text: &str) -> Option<Projection> {
        let latest = comparable_transcript(text);
        let history = self.hypotheses.iter().cloned().collect::<Vec<_>>();
        let (stable_units, stable, unstable) =
            reconcile(&history, self.stable_units, latest.clone())?;
        self.stable_units = stable_units;
        self.hypotheses.push_back(latest);
        while self.hypotheses.len() > HYPOTHESIS_CAPACITY {
            self.hypotheses.pop_front();
        }
        Some(Projection { stable, unstable })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schedules_whole_prefixes_and_latest_wins() {
        let mut engine = BatchEngine::default();
        assert_eq!(engine.on_audio(BATCH_MIN_SAMPLES).unwrap().start_sample, 0);
        assert!(engine
            .on_audio(BATCH_MIN_SAMPLES + BATCH_HOP_SAMPLES)
            .is_none());
        let (_, next) = engine.on_partial_complete(BATCH_MIN_SAMPLES, "one");
        assert_eq!(
            next.unwrap().end_sample,
            BATCH_MIN_SAMPLES + BATCH_HOP_SAMPLES
        );
    }
    #[test]
    fn commit_cancels_partial_and_is_final_priority() {
        let mut engine = BatchEngine::default();
        let _ = engine.on_audio(BATCH_MIN_SAMPLES);
        let final_request = engine.commit(40_000);
        assert_eq!(final_request.kind, DecodeKind::Final);
        assert!(engine.on_audio(50_000).is_none());
    }

    #[test]
    fn a_failed_partial_still_runs_the_latest_queued_prefix() {
        let mut engine = BatchEngine::default();
        let first = engine.on_audio(BATCH_MIN_SAMPLES).unwrap();
        assert!(engine
            .on_audio(BATCH_MIN_SAMPLES + BATCH_HOP_SAMPLES)
            .is_none());
        let next = engine.on_partial_failed(first.end_sample).unwrap();
        assert_eq!(next.end_sample, BATCH_MIN_SAMPLES + BATCH_HOP_SAMPLES);
    }
    #[test]
    fn retains_at_most_three_whole_prefix_hypotheses_without_concatenation() {
        let mut engine = BatchEngine::default();
        for text in [
            "クロードコ",
            "クロードコード",
            "クロードコードです",
            "クロードコードです。",
        ] {
            let projection = engine.apply_hypothesis(text).unwrap();
            assert_eq!(
                format!("{}{}", projection.stable, projection.unstable),
                text
            );
        }
        assert_eq!(engine.hypotheses.len(), 3);
    }
}
