//! Removes a strongly identified copy of our own rendered PCM after system AEC.
//! This never pauses capture: unrelated speech remains in the frame.
use super::converter::CaptureDownsampler;

const RATE: usize = 16_000;
const HISTORY_SAMPLES: usize = RATE / 2;
const MIN_CORRELATION: f32 = 0.82;

pub(super) struct EchoReference {
    downsampler: CaptureDownsampler,
    history: Vec<f32>,
}

impl EchoReference {
    pub(super) fn new() -> Self {
        Self {
            downsampler: CaptureDownsampler::new(),
            history: Vec::with_capacity(HISTORY_SAMPLES),
        }
    }

    pub(super) fn append_rendered(&mut self, rendered_48k: &[f32]) {
        self.downsampler.push(rendered_48k, &mut self.history);
        if self.history.len() > HISTORY_SAMPLES {
            self.history.drain(..self.history.len() - HISTORY_SAMPLES);
        }
    }

    pub(super) fn remove_identified_echo(&self, captured: &mut [f32]) -> bool {
        if captured.is_empty() || self.history.len() < captured.len() {
            return false;
        }
        let capture_energy: f32 = captured.iter().map(|sample| sample * sample).sum();
        if capture_energy < 1e-5 {
            return false;
        }
        let mut best = (0.0_f32, 0.0_f32, 0_usize);
        let last = self.history.len() - captured.len();
        for offset in (0..=last).step_by(4) {
            let reference = &self.history[offset..offset + captured.len()];
            let mut dot = 0.0;
            let mut reference_energy = 0.0;
            for (&input, &rendered) in captured.iter().zip(reference) {
                dot += input * rendered;
                reference_energy += rendered * rendered;
            }
            if reference_energy < 1e-5 {
                continue;
            }
            let correlation = dot.abs() / (capture_energy * reference_energy).sqrt();
            if correlation > best.0 {
                best = (correlation, dot / reference_energy, offset);
            }
        }
        if best.0 < MIN_CORRELATION {
            return false;
        }
        let gain = best.1.clamp(-2.0, 2.0);
        let reference = &self.history[best.2..best.2 + captured.len()];
        for (input, &rendered) in captured.iter_mut().zip(reference) {
            *input -= gain * rendered;
        }
        true
    }
}

/// Outcome of replaying synthetic PCM through the sample-based echo handling.
/// The conversation contract needs both: TTS-only playback must not become an utterance,
/// and a human voice overlapping playback must still reach ASR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EchoContractCheck {
    pub(crate) playback_only_silenced: bool,
    pub(crate) overlapping_speech_preserved: bool,
    pub(crate) unrelated_voice_untouched: bool,
}

#[cfg(test)]
impl EchoContractCheck {
    pub(crate) fn holds(self) -> bool {
        self.playback_only_silenced
            && self.overlapping_speech_preserved
            && self.unrelated_voice_untouched
    }
}

fn rendered_probe(index: usize) -> f32 {
    ((index as f32 * 0.17).sin() + (index as f32 * 0.39).sin() * 0.4) * 0.2
}

fn energy(samples: &[f32]) -> f32 {
    samples.iter().map(|sample| sample * sample).sum()
}

pub(crate) fn contract_self_test() -> EchoContractCheck {
    let rendered: Vec<f32> = (0..4_800).map(|i| rendered_probe(i / 3)).collect();
    let mut filter = EchoReference::new();
    filter.append_rendered(&rendered);

    let mut playback_only: Vec<f32> = filter.history[filter.history.len() - 240..][..160]
        .iter()
        .map(|sample| sample * 0.7)
        .collect();
    let removed = filter.remove_identified_echo(&mut playback_only);
    let rms = (energy(&playback_only) / playback_only.len() as f32).sqrt();
    let playback_only_silenced = removed && rms < 0.006;

    let reference = &filter.history[filter.history.len() - 320..];
    let speech: Vec<f32> = (0..160).map(|i| (i as f32 * 0.071).sin() * 0.02).collect();
    let mut overlapping: Vec<f32> = reference[80..240]
        .iter()
        .zip(&speech)
        .map(|(&echo, &voice)| echo * 0.7 + voice)
        .collect();
    filter.remove_identified_echo(&mut overlapping);
    let (remaining, original) = (energy(&overlapping), energy(&speech));
    let overlapping_speech_preserved = remaining > original * 0.5 && remaining < original * 1.5;

    let mut voice: Vec<f32> = (0..160).map(|i| (i as f32 * 0.071).sin() * 0.08).collect();
    let before = voice.clone();
    let touched = filter.remove_identified_echo(&mut voice);
    let unrelated_voice_untouched = !touched && voice == before;

    EchoContractCheck {
        playback_only_silenced,
        overlapping_speech_preserved,
        unrelated_voice_untouched,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(index: usize) -> f32 {
        ((index as f32 * 0.17).sin() + (index as f32 * 0.39).sin() * 0.4) * 0.2
    }

    #[test]
    fn subtracts_matching_playback_and_preserves_overlapping_speech() {
        let mut filter = EchoReference::new();
        let rendered: Vec<f32> = (0..4_800).map(|i| signal(i / 3)).collect();
        filter.append_rendered(&rendered);
        let reference = &filter.history[filter.history.len() - 320..];
        let speech: Vec<f32> = (0..160).map(|i| (i as f32 * 0.071).sin() * 0.02).collect();
        let mut captured: Vec<f32> = reference[80..240]
            .iter()
            .zip(&speech)
            .map(|(&echo, &voice)| echo * 0.7 + voice)
            .collect();
        assert!(filter.remove_identified_echo(&mut captured));
        let remaining: f32 = captured.iter().map(|sample| sample * sample).sum();
        let original_speech: f32 = speech.iter().map(|sample| sample * sample).sum();
        assert!(remaining < original_speech * 1.5);
    }

    #[test]
    fn playback_only_falls_below_the_conversation_vad_threshold() {
        let mut filter = EchoReference::new();
        let rendered: Vec<f32> = (0..4_800).map(|i| signal(i / 3)).collect();
        filter.append_rendered(&rendered);
        let mut captured: Vec<f32> = filter.history[filter.history.len() - 240..][..160]
            .iter()
            .map(|sample| sample * 0.7)
            .collect();
        assert!(filter.remove_identified_echo(&mut captured));
        let rms = (captured.iter().map(|sample| sample * sample).sum::<f32>()
            / captured.len() as f32)
            .sqrt();
        assert!(rms < 0.006);
    }

    #[test]
    fn self_test_confirms_the_voice_contract() {
        let check = contract_self_test();
        assert!(check.holds(), "{check:?}");
    }

    #[test]
    fn does_not_modify_unrelated_voice() {
        let mut filter = EchoReference::new();
        filter.append_rendered(&(0..4_800).map(|i| signal(i / 3)).collect::<Vec<_>>());
        let mut voice: Vec<f32> = (0..160).map(|i| (i as f32 * 0.071).sin() * 0.08).collect();
        let original = voice.clone();
        assert!(!filter.remove_identified_echo(&mut voice));
        assert_eq!(voice, original);
    }
}
