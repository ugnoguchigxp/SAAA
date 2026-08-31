use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use reqwest::Client;
use zeroize::Zeroizing;

use crate::{CloudAsrProviderSettings, RunCancellation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BatchDecodeOutcome {
    Transcript {
        text: String,
        language: Option<String>,
    },
    NoSpeech,
}

#[async_trait]
pub(crate) trait BatchDecode: Send + Sync {
    async fn decode(
        &self,
        pcm16le: Zeroizing<Vec<u8>>,
        cancellation: Arc<RunCancellation>,
    ) -> Result<BatchDecodeOutcome, String>;
}

#[derive(Clone)]
pub(crate) enum BatchRoute {
    Cloud(CloudAsrProviderSettings),
    LegacyNetwork {
        client: Client,
        endpoint: String,
        model: String,
    },
}

#[derive(Clone)]
pub(crate) struct ProductionBatchDecoder {
    route: BatchRoute,
    timeout_ms: u64,
    allowed_languages: Vec<String>,
    vad_threshold: f32,
}

impl ProductionBatchDecoder {
    pub(crate) fn new(
        route: BatchRoute,
        timeout_ms: u64,
        allowed_languages: Vec<String>,
        vad_threshold: f32,
    ) -> Self {
        Self {
            route,
            timeout_ms,
            allowed_languages,
            vad_threshold,
        }
    }
}

#[async_trait]
impl BatchDecode for ProductionBatchDecoder {
    async fn decode(
        &self,
        pcm16le: Zeroizing<Vec<u8>>,
        cancellation: Arc<RunCancellation>,
    ) -> Result<BatchDecodeOutcome, String> {
        if pcm16le.is_empty() || pcm16le.iter().all(|byte| *byte == 0) {
            return Ok(BatchDecodeOutcome::NoSpeech);
        }
        let samples = Zeroizing::new(
            pcm16le
                .chunks_exact(2)
                .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0)
                .collect::<Vec<_>>(),
        );
        if !contains_speech(&samples, self.vad_threshold) {
            return Ok(BatchDecodeOutcome::NoSpeech);
        }
        let timeout = Duration::from_millis(self.timeout_ms.min(15_000));
        let result = tokio::time::timeout(timeout, async {
            match &self.route {
                BatchRoute::Cloud(provider) => {
                    crate::voice::cloud_asr::transcribe(
                        provider,
                        &samples,
                        16_000,
                        self.timeout_ms.min(15_000),
                        cancellation,
                    )
                    .await
                }
                BatchRoute::LegacyNetwork {
                    client,
                    endpoint,
                    model,
                } => {
                    crate::voice::network_asr::transcribe_at(
                        client,
                        endpoint,
                        &samples,
                        16_000,
                        model,
                        cancellation,
                    )
                    .await
                }
            }
        })
        .await
        .map_err(|_| "ASR request reached its configured timeout".to_string())?;
        match result {
            Ok((text, language)) if !text.trim().is_empty() => {
                crate::voice::language::enforce_allowed_language(
                    language.as_deref(),
                    &self.allowed_languages,
                )?;
                Ok(BatchDecodeOutcome::Transcript { text, language })
            }
            Ok(_) => Ok(BatchDecodeOutcome::NoSpeech),
            Err(error) if error.starts_with("ASR_NO_SPEECH") => Ok(BatchDecodeOutcome::NoSpeech),
            Err(error) => Err(error),
        }
    }
}

pub(crate) fn decoder(
    route: BatchRoute,
    timeout_ms: u64,
    allowed_languages: Vec<String>,
    vad_threshold: f32,
) -> Arc<dyn BatchDecode> {
    Arc::new(ProductionBatchDecoder::new(
        route,
        timeout_ms,
        allowed_languages,
        vad_threshold,
    ))
}

fn contains_speech(samples: &[f32], threshold: f32) -> bool {
    const WINDOW_SAMPLES: usize = 320;
    const MIN_ACTIVE_WINDOWS: usize = 12;
    if samples.len() < 8_000 {
        return false;
    }
    samples
        .chunks_exact(WINDOW_SAMPLES)
        .filter(|window| {
            let rms = (window.iter().map(|sample| sample * sample).sum::<f32>()
                / WINDOW_SAMPLES as f32)
                .sqrt();
            rms.is_finite() && rms >= threshold
        })
        .take(MIN_ACTIVE_WINDOWS)
        .count()
        >= MIN_ACTIVE_WINDOWS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_conversion_uses_the_full_signed_range() {
        let bytes = Zeroizing::new(vec![0x00, 0x80, 0xff, 0x7f]);
        let samples = bytes
            .chunks_exact(2)
            .map(|value| i16::from_le_bytes([value[0], value[1]]) as f32 / 32_768.0)
            .collect::<Vec<_>>();
        assert_eq!(samples, vec![-1.0, 32_767.0 / 32_768.0]);
    }

    #[test]
    fn batch_vad_requires_half_a_second_and_240ms_of_active_audio() {
        let mut samples = vec![0.0; 8_000];
        samples[..11 * 320].fill(0.1);
        assert!(!contains_speech(&samples, 0.008));
        samples[..12 * 320].fill(0.1);
        assert!(contains_speech(&samples, 0.008));
        assert!(!contains_speech(&samples[..7_999], 0.008));
    }
}
