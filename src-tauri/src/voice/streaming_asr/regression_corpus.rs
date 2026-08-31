use std::collections::HashSet;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    harness_stream::{audio_message, decode_provider_event, ProviderEvent, PACKET_BYTES},
    speaker_gate::{release_block, Vote},
};

const SAMPLE_RATE: usize = 16_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CorpusCase {
    id: String,
    category: String,
    seed: u32,
    duration_ms: usize,
    amplitude: i32,
    #[serde(default)]
    boundary_offset_ms: Option<usize>,
    gold: String,
    provider_final: String,
    expected_pcm_sha256: String,
}

fn corpus() -> Vec<CorpusCase> {
    serde_json::from_str(include_str!("../../../testdata/asr_accuracy_corpus.json"))
        .expect("ASR regression corpus is valid")
}

fn fixed_pcm(case: &CorpusCase) -> Vec<u8> {
    let sample_count = SAMPLE_RATE * case.duration_ms / 1_000;
    let mut state = case.seed;
    let mut bytes = Vec::with_capacity(sample_count * 2);
    for index in 0..sample_count {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let noise = ((state >> 16) as u16) as i16 as i32;
        let mut sample = (noise * case.amplitude / i16::MAX as i32)
            .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        if case.category == "click" && index == 800 {
            sample = i16::MAX;
        }
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    for (row, left_character) in left.chars().enumerate() {
        let mut current = Vec::with_capacity(right.len() + 1);
        current.push(row + 1);
        for (column, right_character) in right.iter().enumerate() {
            current.push(
                (current[column] + 1)
                    .min(previous[column + 1] + 1)
                    .min(previous[column] + usize::from(left_character != *right_character)),
            );
        }
        previous = current;
    }
    previous[right.len()]
}

#[test]
fn fixed_pcm_corpus_has_stable_hashes_and_exact_native_packets() {
    let cases = corpus();
    assert_eq!(cases.len(), 9);
    let mut ids = HashSet::new();
    for case in &cases {
        assert!(ids.insert(&case.id));
        assert_eq!(case.duration_ms % 100, 0);
        let pcm = fixed_pcm(case);
        assert_eq!(pcm.len(), SAMPLE_RATE * case.duration_ms / 500);
        assert_eq!(sha256(&pcm), case.expected_pcm_sha256, "{}", case.id);
        for packet in pcm.chunks(PACKET_BYTES) {
            assert_eq!(packet.len(), PACKET_BYTES, "{}", case.id);
            assert!(audio_message(packet.to_vec()).is_ok());
        }
    }
    assert_eq!(
        cases
            .iter()
            .find(|case| case.id == "long-thirty-seconds")
            .unwrap()
            .duration_ms,
        30_000
    );
    assert_eq!(
        cases
            .iter()
            .find(|case| case.id == "fifty-millisecond-boundary")
            .unwrap()
            .boundary_offset_ms,
        Some(50)
    );
}

#[test]
fn controlled_provider_events_match_gold_without_duplicate_finals() {
    let mut final_ids = HashSet::new();
    for case in corpus() {
        if matches!(
            case.category.as_str(),
            "no-speech" | "click" | "speaker-reject"
        ) {
            continue;
        }
        let event = serde_json::json!({
            "type": "final",
            "sessionId": "corpus-session",
            "utteranceId": case.id,
            "revision": 2,
            "startSample": 0,
            "endSample": SAMPLE_RATE * case.duration_ms / 1_000,
            "text": case.provider_final,
            "language": "ja"
        });
        let decoded = decode_provider_event(&serde_json::to_vec(&event).unwrap()).unwrap();
        let ProviderEvent::Final {
            utterance_id, text, ..
        } = decoded
        else {
            panic!("corpus provider must return final")
        };
        assert!(final_ids.insert(utterance_id));
        assert_eq!(edit_distance(&case.gold, &text), 0, "{}", case.id);
    }
}

#[test]
fn quiet_audio_is_preserved_and_rejected_speakers_are_zeroized() {
    for case in corpus() {
        let mut pcm = fixed_pcm(&case);
        match case.category.as_str() {
            "transcript" if case.id == "quiet-speech" => {
                assert!(pcm.iter().any(|byte| *byte != 0));
            }
            "speaker-reject" => {
                let original = pcm.clone();
                let sanitized = release_block(
                    &mut pcm[..PACKET_BYTES],
                    &[Vote::Pass, Vote::Reject, Vote::Reject],
                );
                assert!(pcm[..PACKET_BYTES].iter().all(|byte| *byte == 0));
                assert!(sanitized.iter().all(|byte| *byte == 0));
                assert_ne!(&original[..PACKET_BYTES], sanitized.as_slice());
            }
            _ => {}
        }
    }
}
