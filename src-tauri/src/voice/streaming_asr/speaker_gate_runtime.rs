use std::{collections::VecDeque, sync::Arc, time::Duration};

use zeroize::Zeroizing;

use super::speaker_gate::{release_block, Vote, BLOCK_SAMPLES};
use crate::voice::profile::streaming_verifier::PreparedVoiceVerifier;

const WINDOW_BLOCKS: i64 = 15;
const WINDOW_HOP_BLOCKS: i64 = 5;
const FIRST_WINDOW_START: i64 = -10;
#[cfg(not(test))]
const SCORE_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(test)]
const SCORE_TIMEOUT: Duration = Duration::from_millis(50);

pub(crate) trait SpeakerScorer: Send + Sync {
    fn score(&self, samples_16k: Zeroizing<Vec<f32>>) -> Result<f32, String>;
    fn threshold(&self) -> f32;
}

pub(crate) struct PreparedSpeakerScorer {
    verifier: PreparedVoiceVerifier,
}

impl PreparedSpeakerScorer {
    pub(crate) fn new(verifier: PreparedVoiceVerifier) -> Self {
        Self { verifier }
    }
}

impl SpeakerScorer for PreparedSpeakerScorer {
    fn score(&self, samples_16k: Zeroizing<Vec<f32>>) -> Result<f32, String> {
        self.verifier.score(samples_16k)
    }

    fn threshold(&self) -> f32 {
        self.verifier.threshold()
    }
}

pub(crate) enum SpeakerGate {
    All,
    Target(TargetSpeakerGate),
}

impl SpeakerGate {
    pub(crate) fn new(scorer: Option<Arc<dyn SpeakerScorer>>, vad_threshold: f32) -> Self {
        match scorer {
            Some(scorer) => Self::Target(TargetSpeakerGate::new(scorer, vad_threshold)),
            None => Self::All,
        }
    }

    pub(crate) fn scope(&self) -> &'static str {
        match self {
            Self::All => "all-speakers",
            Self::Target(_) => "target-speaker",
        }
    }

    pub(crate) async fn push(&mut self, packet: Zeroizing<Vec<u8>>) -> Vec<Zeroizing<Vec<u8>>> {
        match self {
            Self::All => vec![packet],
            Self::Target(gate) => gate.push(packet).await,
        }
    }

    pub(crate) async fn flush(&mut self) -> Vec<Zeroizing<Vec<u8>>> {
        match self {
            Self::All => Vec::new(),
            Self::Target(gate) => gate.flush().await,
        }
    }
}

struct PendingBlock {
    index: i64,
    raw: Zeroizing<Vec<u8>>,
    votes: Vec<Vote>,
}

pub(crate) struct TargetSpeakerGate {
    scorer: Arc<dyn SpeakerScorer>,
    vad_threshold: f32,
    blocks: VecDeque<PendingBlock>,
    total_blocks: i64,
    next_window_start: i64,
    score_task: Option<tokio::task::JoinHandle<Result<f32, String>>>,
}

impl TargetSpeakerGate {
    fn new(scorer: Arc<dyn SpeakerScorer>, vad_threshold: f32) -> Self {
        Self {
            scorer,
            vad_threshold,
            blocks: VecDeque::new(),
            total_blocks: 0,
            next_window_start: FIRST_WINDOW_START,
            score_task: None,
        }
    }

    async fn push(&mut self, packet: Zeroizing<Vec<u8>>) -> Vec<Zeroizing<Vec<u8>>> {
        debug_assert_eq!(packet.len(), BLOCK_SAMPLES * 2);
        self.blocks.push_back(PendingBlock {
            index: self.total_blocks,
            raw: packet,
            votes: Vec::with_capacity(3),
        });
        self.total_blocks += 1;
        while self.next_window_start + WINDOW_BLOCKS <= self.total_blocks {
            self.evaluate_next_window().await;
        }
        self.release_ready(false)
    }

    async fn flush(&mut self) -> Vec<Zeroizing<Vec<u8>>> {
        if self.total_blocks == 0 {
            return Vec::new();
        }
        let last_start = ((self.total_blocks - 1) / WINDOW_HOP_BLOCKS) * WINDOW_HOP_BLOCKS;
        while self.next_window_start <= last_start {
            self.evaluate_next_window().await;
        }
        let released = self.release_ready(true);
        self.total_blocks = 0;
        self.next_window_start = FIRST_WINDOW_START;
        released
    }

    async fn evaluate_next_window(&mut self) {
        let start = self.next_window_start;
        let samples = self.window_samples(start);
        let vote = self.score_window(samples).await;
        let end = start + WINDOW_BLOCKS;
        for block in &mut self.blocks {
            if block.index >= start && block.index < end {
                block.votes.push(vote);
            }
        }
        self.next_window_start += WINDOW_HOP_BLOCKS;
    }

    fn window_samples(&self, start: i64) -> Zeroizing<Vec<f32>> {
        let mut samples = Zeroizing::new(vec![0.0; WINDOW_BLOCKS as usize * BLOCK_SAMPLES]);
        for block in &self.blocks {
            let offset = block.index - start;
            if !(0..WINDOW_BLOCKS).contains(&offset) {
                continue;
            }
            let destination = offset as usize * BLOCK_SAMPLES;
            for (sample_index, bytes) in block.raw.chunks_exact(2).enumerate() {
                samples[destination + sample_index] =
                    i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0;
            }
        }
        samples
    }

    async fn score_window(&mut self, samples: Zeroizing<Vec<f32>>) -> Vote {
        let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
            / samples.len() as f32)
            .sqrt();
        if !rms.is_finite() || rms < self.vad_threshold {
            return Vote::Reject;
        }
        if let Some(task) = self.score_task.take() {
            if task.is_finished() {
                let _ = task.await;
            } else {
                self.score_task = Some(task);
                return Vote::Reject;
            }
        }
        let scorer = self.scorer.clone();
        let threshold = scorer.threshold();
        let mut task = tokio::task::spawn_blocking(move || scorer.score(samples));
        let result = tokio::time::timeout(SCORE_TIMEOUT, &mut task).await;
        match result {
            Ok(Ok(Ok(score))) if score.is_finite() && score >= threshold => Vote::Pass,
            Err(_) => {
                // spawn_blocking tasks cannot be cancelled once running. Retaining the
                // timed-out handle prevents a hanging verifier from accumulating work.
                self.score_task = Some(task);
                Vote::Reject
            }
            _ => Vote::Reject,
        }
    }

    fn release_ready(&mut self, flushing: bool) -> Vec<Zeroizing<Vec<u8>>> {
        let mut released = Vec::new();
        while self
            .blocks
            .front()
            .is_some_and(|block| flushing || block.votes.len() >= 3)
        {
            let mut block = self.blocks.pop_front().expect("front was checked");
            let sanitized = release_block(&mut block.raw, &block.votes);
            released.push(Zeroizing::new(sanitized));
        }
        released
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct FixedScorer(f32);
    impl SpeakerScorer for FixedScorer {
        fn score(&self, _samples_16k: Zeroizing<Vec<f32>>) -> Result<f32, String> {
            Ok(self.0)
        }
        fn threshold(&self) -> f32 {
            0.5
        }
    }

    fn voiced_packet() -> Zeroizing<Vec<u8>> {
        let sample = 5_000_i16.to_le_bytes();
        Zeroizing::new(sample.repeat(BLOCK_SAMPLES))
    }

    #[tokio::test]
    async fn waits_for_three_overlapping_votes_then_releases_in_order() {
        let mut gate = SpeakerGate::new(Some(Arc::new(FixedScorer(0.9))), 0.001);
        let mut output = Vec::new();
        for _ in 0..15 {
            output.extend(gate.push(voiced_packet()).await);
        }
        assert_eq!(output.len(), 5);
        assert!(output
            .iter()
            .all(|packet| packet.iter().any(|byte| *byte != 0)));
        output.extend(gate.flush().await);
        assert_eq!(output.len(), 15);
    }

    #[tokio::test]
    async fn rejection_zeroizes_every_raw_block() {
        let mut gate = SpeakerGate::new(Some(Arc::new(FixedScorer(0.1))), 0.001);
        for _ in 0..3 {
            assert!(gate.push(voiced_packet()).await.is_empty());
        }
        let output = gate.flush().await;
        assert_eq!(output.len(), 3);
        assert!(output
            .iter()
            .all(|packet| packet.iter().all(|byte| *byte == 0)));
    }

    struct SlowScorer(Arc<AtomicUsize>);
    impl SpeakerScorer for SlowScorer {
        fn score(&self, _samples_16k: Zeroizing<Vec<f32>>) -> Result<f32, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(200));
            Ok(0.9)
        }
        fn threshold(&self) -> f32 {
            0.5
        }
    }

    #[tokio::test]
    async fn timed_out_verifier_never_accumulates_blocking_tasks() {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut gate = SpeakerGate::new(Some(Arc::new(SlowScorer(calls.clone()))), 0.001);
        let mut output = Vec::new();
        for _ in 0..15 {
            output.extend(gate.push(voiced_packet()).await);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(output.len(), 5);
        assert!(output
            .iter()
            .all(|packet| packet.iter().all(|byte| *byte == 0)));
    }
}
