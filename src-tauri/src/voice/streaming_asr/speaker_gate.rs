//! Local-only temporal speaker gate. Rejected blocks are replaced before any ASR adapter sees them.
pub(crate) const BLOCK_SAMPLES: usize = 1_600;
const VOTES_REQUIRED: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Vote {
    Pass,
    Reject,
}
pub(crate) fn release_block(raw_pcm16le: &mut [u8], votes: &[Vote]) -> Vec<u8> {
    let accepted = votes.iter().filter(|vote| **vote == Vote::Pass).count() >= VOTES_REQUIRED;
    if accepted {
        return raw_pcm16le.to_vec();
    }
    raw_pcm16le.fill(0);
    vec![0; raw_pcm16le.len()]
}
pub(crate) fn block_accepts(scores: &[Result<f32, ()>], threshold: f32) -> bool {
    scores
        .iter()
        .filter(|score| matches!(score, Ok(value) if *value >= threshold))
        .count()
        >= VOTES_REQUIRED
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_error_votes_and_zeroizes_raw_audio() {
        let mut raw = vec![7; BLOCK_SAMPLES * 2];
        let sanitized = release_block(&mut raw, &[Vote::Pass, Vote::Reject, Vote::Reject]);
        assert!(raw.iter().all(|b| *b == 0));
        assert!(sanitized.iter().all(|b| *b == 0));
        assert!(!block_accepts(&[Ok(0.7), Err(()), Ok(0.1)], 0.55));
    }
}
