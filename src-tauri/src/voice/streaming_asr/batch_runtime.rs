use std::sync::Arc;

use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::RunCancellation;

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

pub(crate) struct ProductionBatchDecoder {
    readers: crate::persistence::SqliteReaders,
    conversation: String,
    network: crate::voice::network_asr::NetworkAsrRuntime,
}

#[async_trait]
impl BatchDecode for ProductionBatchDecoder {
    async fn decode(
        &self,
        pcm16le: Zeroizing<Vec<u8>>,
        cancellation: Arc<RunCancellation>,
    ) -> Result<BatchDecodeOutcome, String> {
        let selected = self.readers.read(|c| {
            let mut selected = crate::voice::session::select_streaming_asr(c)?;
            if crate::larm_voice::enabled()
                && matches!(selected.route, crate::voice::session::AsrRoute::Harness(_))
            {
                selected.route = crate::voice::session::AsrRoute::Larm(
                    self.conversation.clone(),
                    crate::persistence::load_model_providers(c)?.harness,
                );
            }
            Ok(selected)
        })?;
        if pcm16le.is_empty() || pcm16le.iter().all(|byte| *byte == 0) {
            return Ok(BatchDecodeOutcome::NoSpeech);
        }
        let samples = Zeroizing::new(
            pcm16le
                .chunks_exact(2)
                .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0)
                .collect::<Vec<_>>(),
        );
        if !contains_speech(
            &samples,
            crate::voice::session::vad_rms_threshold(&selected.vad_sensitivity),
        ) {
            return Ok(BatchDecodeOutcome::NoSpeech);
        }
        let result = crate::voice::session::asr_routes::transcribe_routes(
            &self.network,
            &samples,
            16_000,
            &selected,
            cancellation,
        )
        .await;
        match result {
            Ok((text, language)) if !text.trim().is_empty() => {
                crate::voice::language::enforce_allowed_language(
                    language.as_deref(),
                    &selected.allowed_languages,
                )?;
                Ok(BatchDecodeOutcome::Transcript { text, language })
            }
            Ok(_) => Ok(BatchDecodeOutcome::NoSpeech),
            Err(error) if is_no_speech_error(&error) => Ok(BatchDecodeOutcome::NoSpeech),
            Err(error) => Err(error),
        }
    }
}

fn is_no_speech_error(error: &str) -> bool {
    error.starts_with("ASR_NO_SPEECH")
}

pub(crate) fn decoder(
    readers: crate::persistence::SqliteReaders,
    conversation: String,
) -> Result<Arc<dyn BatchDecode>, String> {
    Ok(Arc::new(ProductionBatchDecoder {
        readers,
        conversation,
        network: crate::voice::network_asr::NetworkAsrRuntime::new()?,
    }))
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

    #[test]
    fn empty_provider_transcripts_are_no_speech_not_provider_failures() {
        assert!(is_no_speech_error(
            "ASR_NO_SPEECH: LAN ASR completed without a transcript"
        ));
        assert!(is_no_speech_error(
            "ASR_NO_SPEECH: Cloud ASR completed without a transcript"
        ));
        assert!(!is_no_speech_error("LAN ASR returned HTTP 503"));
    }
}
