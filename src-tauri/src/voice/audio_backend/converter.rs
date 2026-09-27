//! Worker-thread resamplers. Callbacks never call these.
pub const VPIO_RATE: u32 = 48_000;

pub fn i16_to_f32_mono(samples: &[i16], channels: u16, output: &mut Vec<f32>) {
    output.clear();
    if channels <= 1 {
        output.extend(samples.iter().map(|sample| *sample as f32 / 32768.0));
        return;
    }
    let channels = channels as usize;
    for frame in samples.chunks(channels) {
        if frame.len() < channels {
            break;
        }
        let sum: f32 = frame.iter().map(|sample| *sample as f32 / 32768.0).sum();
        output.push(sum / channels as f32);
    }
}

fn sinc_taps(tap_count: usize, cutoff: f32) -> Vec<f32> {
    let mut taps = vec![0.0; tap_count];
    let mid = (tap_count - 1) as f32 / 2.0;
    let mut sum = 0.0;
    for (index, tap) in taps.iter_mut().enumerate() {
        let x = index as f32 - mid;
        let sinc = if x.abs() < f32::EPSILON {
            1.0
        } else {
            let omega = 2.0 * std::f32::consts::PI * cutoff * x;
            omega.sin() / (std::f32::consts::PI * x)
        };
        let window = 0.54
            - 0.46 * (2.0 * std::f32::consts::PI * index as f32 / (tap_count as f32 - 1.0)).cos();
        *tap = sinc * window;
        sum += *tap;
    }
    if sum.abs() > f32::EPSILON {
        for tap in &mut taps {
            *tap /= sum;
        }
    }
    taps
}

/// FIR then take every third sample: 48 kHz → 16 kHz.
pub struct CaptureDownsampler {
    fir: Vec<f32>,
    history: Vec<f32>,
    phase: usize,
}

impl CaptureDownsampler {
    pub fn new() -> Self {
        Self {
            fir: sinc_taps(15, 1.0 / 6.0),
            history: vec![0.0; 15],
            phase: 0,
        }
    }

    pub fn push(&mut self, input: &[f32], output: &mut Vec<f32>) {
        for &sample in input {
            self.history.copy_within(1.., 0);
            if let Some(last) = self.history.last_mut() {
                *last = sample;
            }
            if self.phase == 0 {
                let mut acc = 0.0;
                for (tap, value) in self.fir.iter().zip(self.history.iter()) {
                    acc += tap * value;
                }
                output.push(acc);
            }
            self.phase = (self.phase + 1) % 3;
        }
    }
}

pub struct LinearResampler {
    from: u32,
    to: u32,
    prev: f32,
    frac: f64,
    primed: bool,
}

impl LinearResampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self {
            from: from.max(1),
            to: to.max(1),
            prev: 0.0,
            frac: 0.0,
            primed: false,
        }
    }

    pub fn from_rate(&self) -> u32 {
        self.from
    }

    pub fn reset(&mut self, from: u32) {
        self.from = from.max(1);
        self.frac = 0.0;
        self.primed = false;
        self.prev = 0.0;
    }

    pub fn push(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if input.is_empty() {
            return;
        }
        if self.from == self.to {
            output.extend_from_slice(input);
            return;
        }
        let step = self.from as f64 / self.to as f64;
        if !self.primed {
            self.prev = input[0];
            self.primed = true;
        }
        let mut pos = self.frac;
        let mut index = 0usize;
        let mut current = self.prev;
        let mut next = input[0];
        while index < input.len() {
            while pos < 1.0 {
                let t = pos as f32;
                output.push(current + (next - current) * t);
                pos += step;
            }
            pos -= 1.0;
            current = next;
            index += 1;
            if index < input.len() {
                next = input[index];
            }
        }
        self.prev = input.last().copied().unwrap_or(current);
        self.frac = pos;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsamples_silence_to_the_asr_rate() {
        let mut decimator = CaptureDownsampler::new();
        let input = vec![0.0; 480];
        let mut output = Vec::new();
        decimator.push(&input, &mut output);
        assert_eq!(output.len(), 160);
        assert!(output.iter().all(|sample| sample.abs() < 1e-6));
    }

    #[test]
    fn upsamples_a_constant_tone_to_48k() {
        let mut resampler = LinearResampler::new(24_000, VPIO_RATE);
        let input = vec![0.25; 240];
        let mut output = Vec::new();
        resampler.push(&input, &mut output);
        assert!(output.len() >= 470);
        assert!(output.iter().all(|sample| (*sample - 0.25).abs() < 0.05));
    }

    #[test]
    fn mixes_stereo_i16_to_mono_float() {
        let mut output = Vec::new();
        i16_to_f32_mono(&[16_384, -16_384], 2, &mut output);
        assert_eq!(output.len(), 1);
        assert!(output[0].abs() < 1e-4);
    }
}
