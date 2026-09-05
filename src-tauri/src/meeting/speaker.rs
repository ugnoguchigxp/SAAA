use std::{sync::Arc, time::Duration};
use zeroize::Zeroizing;

use super::SegmentInput;
use crate::{voice::streaming_asr::speaker_gate_runtime::SpeakerScorer, AppState};

#[cfg(not(test))]
const SCORE_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(test)]
const SCORE_TIMEOUT: Duration = Duration::from_millis(50);

pub(super) struct SpeakerFilter {
    scorer: Arc<dyn SpeakerScorer>,
    capacity: Arc<tokio::sync::Semaphore>,
}

impl SpeakerFilter {
    pub(super) fn new(scorer: Arc<dyn SpeakerScorer>) -> Self {
        Self {
            scorer,
            capacity: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    async fn accepts(&self, samples: Zeroizing<Vec<f32>>) -> bool {
        let Ok(permit) = self.capacity.clone().try_acquire_owned() else {
            return false;
        };
        let scorer = self.scorer.clone();
        let threshold = scorer.threshold();
        let mut task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            scorer.score(samples)
        });
        matches!(tokio::time::timeout(SCORE_TIMEOUT, &mut task).await, Ok(Ok(Ok(value))) if value.is_finite() && value >= threshold)
    }
}

pub(super) async fn matches(state: &AppState, input: &SegmentInput, samples: &[f32]) -> bool {
    match state.meeting.speaker_filter(input) {
        Ok(None) => true,
        Ok(Some(filter)) => filter.accepts(Zeroizing::new(samples.to_vec())).await,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FixedScorer(Result<f32, String>);
    impl SpeakerScorer for FixedScorer {
        fn score(&self, _samples: Zeroizing<Vec<f32>>) -> Result<f32, String> {
            self.0.clone()
        }
        fn threshold(&self) -> f32 {
            0.5
        }
    }
    struct SlowScorer(Arc<AtomicUsize>);
    impl SpeakerScorer for SlowScorer {
        fn score(&self, _samples: Zeroizing<Vec<f32>>) -> Result<f32, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(100));
            Ok(0.9)
        }
        fn threshold(&self) -> f32 {
            0.5
        }
    }
    #[tokio::test]
    async fn rejects_mismatch_and_errors_before_transport() {
        let samples = Zeroizing::new(vec![0.1; 16_000]);
        assert!(
            SpeakerFilter::new(Arc::new(FixedScorer(Ok(0.9))))
                .accepts(samples.clone())
                .await
        );
        assert!(
            !SpeakerFilter::new(Arc::new(FixedScorer(Ok(0.2))))
                .accepts(samples.clone())
                .await
        );
        assert!(
            !SpeakerFilter::new(Arc::new(FixedScorer(Err("invalid".into()))))
                .accepts(samples)
                .await
        );
    }

    #[tokio::test]
    async fn a_timed_out_score_does_not_start_another_blocking_task() {
        let calls = Arc::new(AtomicUsize::new(0));
        let filter = SpeakerFilter::new(Arc::new(SlowScorer(calls.clone())));
        assert!(!filter.accepts(Zeroizing::new(vec![0.1; 16_000])).await);
        assert!(!filter.accepts(Zeroizing::new(vec![0.1; 16_000])).await);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
