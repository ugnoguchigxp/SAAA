#[path = "../src/runtime/conversation_check/speech_projection.rs"]
mod speech_projection;
#[path = "../src/voice/streaming_tts/chunker.rs"]
mod tts_chunker;
#[path = "../src/voice_text.rs"]
mod voice_text;

use speech_projection::SpeechProjection;
use tts_chunker::{SelectReason, SentenceAccumulator};

#[test]
fn speech_projection_blocks_split_thinking_tag_without_losing_normal_less_than() {
    let mut projection = SpeechProjection::default();
    assert_eq!(projection.append("値は 3 <"), "値は 3 ");
    assert_eq!(projection.append(" 5 です。"), "< 5 です。");
    assert!(!projection.blocked());
    assert_eq!(projection.append("答えです。<thi"), "答えです。");
    assert_eq!(projection.append("nk>秘密"), "");
    assert!(projection.blocked());
    assert_eq!(projection.append("続き"), "");

    let mut control = SpeechProjection::default();
    assert_eq!(control.append("前半<"), "前半");
    assert_eq!(control.append("|im_start|>"), "");
    assert!(control.blocked());
}

#[test]
fn first_complete_sentence_is_ready_before_llm_finishes() {
    let mut stream = SentenceAccumulator::default();
    stream.append("今日は良い天気です。").unwrap();
    let first = stream
        .next_chunk(SelectReason::Append)
        .expect("first sentence");
    assert_eq!(first.spoken, "今日は良い天気です。");
    stream.append("続きの説明をします。").unwrap();
    stream
        .finish("今日は良い天気です。続きの説明をします。")
        .unwrap();
    let rest = stream
        .next_chunk(SelectReason::Completion)
        .expect("remaining sentence");
    assert_eq!(rest.spoken, "続きの説明をします。");
    assert!(stream.next_chunk(SelectReason::Completion).is_none());
}

#[test]
fn an_idle_stream_can_release_a_long_clause_at_a_comma() {
    let mut stream = SentenceAccumulator::default();
    stream
        .append("これは少し長い前置きですが、次に続きます")
        .unwrap();
    assert!(stream.next_chunk(SelectReason::Append).is_none());
    let first = stream.next_chunk(SelectReason::Idle).expect("idle clause");
    assert_eq!(first.spoken, "これは少し長い前置きですが、");
}

#[test]
fn unpunctuated_text_is_bounded_before_completion() {
    let mut stream = SentenceAccumulator::default();
    stream.append(&"あ".repeat(600)).unwrap();
    let first = stream
        .next_chunk(SelectReason::Append)
        .expect("bounded chunk");
    assert_eq!(first.spoken.chars().count(), 240);
    assert_eq!(first.raw, "あ".repeat(240));
}

#[test]
fn arbitrary_delta_splits_keep_the_same_spoken_content() {
    let answer = "今日は3.14を確認します。詳しくは[資料](https://example.com)をご覧ください。";
    let expected = voice_text::text_for_speech(answer);
    for split in answer
        .char_indices()
        .map(|(index, _)| index)
        .chain([answer.len()])
    {
        let mut stream = SentenceAccumulator::default();
        stream.append(&answer[..split]).unwrap();
        let mut spoken = Vec::new();
        while let Some(chunk) = stream.next_chunk(SelectReason::Append) {
            spoken.push(chunk.spoken);
        }
        stream.append(&answer[split..]).unwrap();
        while let Some(chunk) = stream.next_chunk(SelectReason::Append) {
            spoken.push(chunk.spoken);
        }
        stream.finish(answer).unwrap();
        while let Some(chunk) = stream.next_chunk(SelectReason::Completion) {
            spoken.push(chunk.spoken);
        }
        let compact = |text: &str| {
            text.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
        };
        assert_eq!(compact(&spoken.concat()), compact(&expected));
    }
}
