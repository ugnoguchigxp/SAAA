//! Apply the configured local speaker gate before either conversation ASR transport.
use super::streaming_asr::speaker_gate_runtime::{PreparedSpeakerScorer, SpeakerGate};
use std::sync::Arc;
use zeroize::Zeroizing;

pub(crate) fn prepare(state: &crate::AppState) -> Result<SpeakerGate, String> {
    state
        .voice_profile
        .read_with_snapshot(&state.sqlite_readers, |connection, _| {
            let verifier = state.voice_profile.prepare_streaming_verifier(connection)?;
            Ok(SpeakerGate::new(
                verifier.map(|v| Arc::new(PreparedSpeakerScorer::new(v)) as _),
                0.008,
            ))
        })
}

pub(crate) async fn filter_samples(gate: &mut SpeakerGate, samples: Vec<f32>) -> Vec<f32> {
    if gate.scope() == "all-speakers" {
        return samples;
    }
    let samples = Zeroizing::new(samples);
    let mut result = Vec::with_capacity(samples.len());
    for block in samples.chunks(1600) {
        let mut packet = Zeroizing::new(vec![0; 3200]);
        for (i, sample) in block.iter().enumerate() {
            let pcm = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            packet[2 * i..2 * i + 2].copy_from_slice(&pcm.to_le_bytes());
        }
        append(&mut result, gate.push(packet).await);
    }
    append(&mut result, gate.flush().await);
    result.truncate(samples.len());
    result
}

fn append(result: &mut Vec<f32>, packets: Vec<Zeroizing<Vec<u8>>>) {
    for packet in packets {
        result.extend(
            packet
                .chunks_exact(2)
                .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / i16::MAX as f32),
        );
    }
}

#[cfg(feature = "provider-unit-test-harness")]
pub(crate) async fn verify_gate_fixture() {
    use super::streaming_asr::speaker_gate_runtime::SpeakerScorer;
    struct Score(f32);
    impl SpeakerScorer for Score {
        fn score(&self, _: Zeroizing<Vec<f32>>) -> Result<f32, String> {
            Ok(self.0)
        }
        fn threshold(&self) -> f32 {
            0.7
        }
    }
    let samples = vec![0.2; 24_317];
    let mut disabled = SpeakerGate::new(None, 0.008);
    assert_eq!(
        filter_samples(&mut disabled, samples.clone()).await,
        samples
    );
    let mut rejected = SpeakerGate::new(Some(Arc::new(Score(0.1))), 0.008);
    let filtered = filter_samples(&mut rejected, samples.clone()).await;
    assert_eq!(filtered.len(), samples.len());
    assert!(filtered.iter().all(|sample| *sample == 0.0));
    let mut accepted = SpeakerGate::new(Some(Arc::new(Score(0.9))), 0.008);
    let filtered = filter_samples(&mut accepted, samples.clone()).await;
    assert_eq!(filtered.len(), samples.len());
    assert!(filtered.iter().all(|sample| (*sample - 0.2).abs() < 0.0001));
}
