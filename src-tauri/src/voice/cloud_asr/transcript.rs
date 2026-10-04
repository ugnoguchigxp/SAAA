use super::*;

pub(super) fn select_transcript(
    result: TranscriptionResponse,
    preserve_full_text: bool,
) -> Result<(String, Option<String>), String> {
    let language = result.detected_language();
    if response_is_no_speech(&result.segments) {
        return Err("ASR_NO_SPEECH: The ASR service classified the audio as non-speech".into());
    }
    let text = if preserve_full_text && result.text.trim().is_empty() {
        result
            .segments
            .iter()
            .filter_map(|segment| segment.text.as_deref())
            .collect::<String>()
    } else {
        result.text
    };
    if text.trim().is_empty() {
        return Err("ASR_NO_SPEECH: Cloud ASR completed without a transcript".into());
    }
    let text = if preserve_full_text {
        text
    } else {
        bounded_text(text.trim(), 16_000)
    };
    Ok((text, language))
}

pub(super) fn response_is_no_speech(segments: &[TranscriptionSegment]) -> bool {
    !segments.is_empty()
        && segments.iter().all(|segment| {
            segment
                .no_speech_prob
                .is_some_and(|probability| probability.is_finite() && probability >= 0.6)
        })
}
