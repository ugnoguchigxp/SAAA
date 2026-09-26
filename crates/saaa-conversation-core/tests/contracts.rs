use saaa_conversation_core::{
    contracts::{validate_input, Action, InputError, TextInput},
    frontdesk::{Frontdesk, State as FrontdeskState, DELEGATE_EXPLANATION},
    qwen::{CompletionDecoder, ControlParser, Error as QwenError, Event, SseDecoder},
    speaking::{Speaking, State as SpeechState},
};

#[test]
fn input_limits_are_utf8_bytes() {
    let mut input = TextInput {
        session_id: "session-1".into(),
        input_id: "input_1".into(),
        text: "あ".repeat(1366),
    };
    assert_eq!(validate_input(&input), Err(InputError::TooLong));
    input.text = "   ".into();
    assert_eq!(validate_input(&input), Err(InputError::Empty));
    input.text = "本文".into();
    assert_eq!(validate_input(&input), Ok(()));
}

#[test]
fn control_is_strict_at_every_character_boundary() {
    let source = "{\n \"action\": \"reply\"\n}\nおはようございます。";
    for (split, _) in source.char_indices().skip(1) {
        let mut parser = ControlParser::default();
        let mut events = parser.push(&source[..split]).unwrap();
        events.extend(parser.push(&source[split..]).unwrap());
        assert_eq!(events.remove(0), Event::Control(Action::Reply));
        assert_eq!(
            events
                .into_iter()
                .map(|event| match event {
                    Event::Body(body) => body,
                    Event::Control(_) => panic!("second control"),
                })
                .collect::<String>(),
            "おはようございます。"
        );
        assert_eq!(parser.finish(), Ok(()));
    }
}

#[test]
fn rejects_duplicate_unknown_and_second_control() {
    let mut parser = ControlParser::default();
    assert_eq!(
        parser.push(" {\"action\":\"reply\"}"),
        Err(QwenError::InvalidPrefix)
    );
    for control in [
        r#"{"action":"reply","action":"delegate"}"#,
        r#"{"action":"reply","other":1}"#,
        r#"{"action":"unknown"}"#,
    ] {
        let mut parser = ControlParser::default();
        assert_eq!(parser.push(control), Err(QwenError::InvalidControl));
    }
    let mut parser = ControlParser::default();
    assert_eq!(
        parser.push("{\"action\":\"reply\"}\n{\"action\":\"delegate\"}\n本文"),
        Err(QwenError::DuplicateControl)
    );
    let mut parser = ControlParser::default();
    parser.push("{\"action\":\"reply\"}\n{").unwrap();
    assert_eq!(
        parser.push("  \"action\":\"delegate\"}\n本文"),
        Err(QwenError::DuplicateControl)
    );
}

#[test]
fn sse_survives_every_byte_boundary() {
    let source = "\u{feff}: note\r\ndata: 日本\r\ndata: 語\r\n\r\ndata: [DONE]\n\n";
    for split in 0..=source.len() {
        let mut decoder = SseDecoder::default();
        let mut events = decoder.push(&source.as_bytes()[..split]).unwrap();
        events.extend(decoder.push(&source.as_bytes()[split..]).unwrap());
        assert_eq!(events, ["日本\n語", "[DONE]"]);
    }
}

#[test]
fn completion_requires_stop_and_done_and_never_exposes_reasoning() {
    let mut decoder = CompletionDecoder::default();
    let delta = r#"{"choices":[{"index":0,"delta":{"reasoning_content":"secret","content":"本文"},"finish_reason":null}]}"#;
    assert_eq!(decoder.push_event(delta), Ok(Some((0, "本文".into()))));
    assert_eq!(decoder.finish_stream(), Err(QwenError::ProviderFinish));
    let stop = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
    assert_eq!(decoder.push_event(stop), Ok(None));
    assert_eq!(decoder.finish_stream(), Err(QwenError::ProviderFinish));
    assert_eq!(decoder.push_event("[DONE]"), Ok(None));
    assert_eq!(decoder.finish_stream(), Ok(()));
}

#[test]
fn completion_rejects_length_tool_and_other_choice() {
    for event in [
        r#"{"choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[]},"finish_reason":null}]}"#,
    ] {
        assert_eq!(
            CompletionDecoder::default().push_event(event),
            Err(QwenError::ProviderFinish)
        );
    }
    let other = r#"{"choices":[{"index":1,"delta":{"content":"hidden"},"finish_reason":null}]}"#;
    assert_eq!(
        CompletionDecoder::default().push_event(other),
        Err(QwenError::ProviderProtocol)
    );
}

#[test]
fn delegate_never_adopts_model_body() {
    let mut frontdesk = Frontdesk::default();
    assert_eq!(
        frontdesk.adopt_control(Action::Delegate),
        Ok(Some(DELEGATE_EXPLANATION))
    );
    assert_eq!(frontdesk.state(), FrontdeskState::Unsupported);
    assert!(frontdesk.append("モデルが仕事を受け付けました").is_err());
    assert_eq!(frontdesk.public_text(), DELEGATE_EXPLANATION);
}

#[test]
fn cancelled_frontdesk_rejects_late_body_and_completion() {
    let mut frontdesk = Frontdesk::default();
    frontdesk.adopt_control(Action::Reply).unwrap();
    frontdesk.append("保存前の本文").unwrap();
    frontdesk.cancel();
    assert_eq!(frontdesk.state(), FrontdeskState::Cancelled);
    assert!(frontdesk.append("遅着").is_err());
    assert!(frontdesk.complete().is_err());
}

#[test]
fn speech_starts_on_first_clause_and_flushes_tail_only_on_success() {
    let mut speech = Speaking::default();
    speech.start().unwrap();
    let clauses = speech.push("おはよう。続き").unwrap();
    assert_eq!(clauses[0].text, "おはよう。");
    let first = speech.begin_next().unwrap().unwrap();
    assert_eq!(first.index, 0);
    speech.playback_started(0, 0).unwrap();
    speech.playback_ended(0, 0).unwrap();
    assert_eq!(speech.last_played(), Some(0));
    assert_eq!(speech.finish_body().unwrap().unwrap().text, "続き");
    speech.begin_next().unwrap();
    speech.playback_started(1, 0).unwrap();
    speech.playback_ended(1, 0).unwrap();
    assert_eq!(speech.state(), SpeechState::Played);
}

#[test]
fn stopped_speech_rejects_late_receipts_and_deltas() {
    let mut speech = Speaking::default();
    speech.start().unwrap();
    speech.push("こんにちは。").unwrap();
    speech.begin_next().unwrap();
    assert!(speech.stop());
    assert_eq!(speech.state(), SpeechState::Stopping);
    assert!(speech.playback_started(0, 0).is_err());
    speech.confirm_stopped().unwrap();
    assert!(speech.push("続き。").is_err());
    assert!(!speech.stop());
}
