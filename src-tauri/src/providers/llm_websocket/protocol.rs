use std::collections::HashSet;

use serde::{
    de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const SUBPROTOCOL: &str = "saaa.llm-stream.v1";
pub(crate) const MAX_DELTA_BYTES: usize = 16_384;
pub(crate) const MAX_CONTENT_BYTES: usize = 262_144;
pub(crate) const MAX_CONTENT_CHARS: usize = 64_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunStart<'a> {
    #[serde(rename = "type")]
    pub(crate) message_type: &'static str,
    pub(crate) run_id: &'a str,
    pub(crate) allocation_id: &'a str,
    pub(crate) model: &'a str,
    pub(crate) messages: &'a [Value],
    pub(crate) tools: &'a [Value],
    pub(crate) reasoning: Reasoning<'a>,
    pub(crate) max_output_tokens: u32,
    pub(crate) max_tool_calls: u8,
    pub(crate) timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(crate) struct Reasoning<'a> {
    pub(crate) effort: &'a str,
}

impl<'a> RunStart<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        run_id: &'a str,
        allocation_id: &'a str,
        model: &'a str,
        messages: &'a [Value],
        tools: &'a [Value],
        reasoning_effort: &'a str,
        max_output_tokens: u32,
        timeout_ms: u64,
    ) -> Self {
        let reasoning_effort = match reasoning_effort {
            "none" | "low" | "medium" | "high" => reasoning_effort,
            "xhigh" => "high",
            _ => "medium",
        };
        Self {
            message_type: "run.start",
            run_id,
            allocation_id,
            model,
            messages,
            tools,
            reasoning: Reasoning {
                effort: reasoning_effort,
            },
            max_output_tokens,
            max_tool_calls: if tools.is_empty() { 0 } else { 32 },
            timeout_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunAck<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    run_id: &'a str,
    ack_seq: u64,
    content_sha256: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunResume<'a> {
    #[serde(rename = "type")]
    message_type: &'static str,
    run_id: &'a str,
    allocation_id: &'a str,
    ack_seq: u64,
    content_sha256: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AcknowledgedCheckpoint {
    pub(crate) seq: u64,
    pub(crate) content_sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ProviderEvent {
    Accepted {
        seq: u64,
    },
    Delta {
        seq: u64,
        text: String,
    },
    ToolCall {
        seq: u64,
        call_id: String,
        name: String,
        arguments: Value,
    },
    Completed {
        seq: u64,
        finish_reason: FinishReason,
    },
    Failed {
        seq: u64,
        code: String,
        output_started: bool,
    },
    Cancelled {
        seq: u64,
        output_started: bool,
    },
    Duplicate {
        seq: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinishReason {
    Stop,
    Length,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeNegotiation {
    Resumed,
}

pub(crate) fn validate_resume_negotiation(
    text: &str,
    run_id: &str,
    expected_ack_seq: u64,
) -> Result<ResumeNegotiation, ProtocolError> {
    reject_duplicate_keys(text)?;
    let envelope: Envelope = serde_json::from_str(text).map_err(|_| ProtocolError::Json)?;
    match envelope.message_type.as_str() {
        "run.resumed" => {
            let message: RunResumed = parse(text)?;
            if message.run_id != run_id || message.ack_seq != expected_ack_seq {
                return Err(ProtocolError::StaleRun);
            }
            Ok(ResumeNegotiation::Resumed)
        }
        _ => Err(ProtocolError::Schema),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProtocolError {
    Framing,
    Json,
    Schema,
    StaleRun,
    SequenceGap,
    Terminal,
    ContentLimit,
    ContentIntegrity,
}

pub(crate) struct OrderedRun {
    run_id: String,
    expected_seq: u64,
    last_accepted_seq: u64,
    content: String,
    content_chars: usize,
    hasher: Sha256,
    terminal: bool,
}

impl OrderedRun {
    pub(crate) fn new(run_id: &str) -> Result<Self, ProtocolError> {
        if !valid_identifier(run_id) {
            return Err(ProtocolError::Schema);
        }
        Ok(Self {
            run_id: run_id.to_string(),
            expected_seq: 1,
            last_accepted_seq: 0,
            content: String::new(),
            content_chars: 0,
            hasher: Sha256::new(),
            terminal: false,
        })
    }

    pub(crate) fn accept_binary(&mut self, frame: &[u8]) -> Result<ProviderEvent, ProtocolError> {
        self.ensure_accepted()?;
        if frame.len() <= 16
            || frame.len() > 16 + MAX_DELTA_BYTES
            || &frame[..4] != b"SAD1"
            || frame[4] != 1
            || frame[5] != 0
            || u16::from_be_bytes([frame[6], frame[7]]) != 16
        {
            return Err(ProtocolError::Framing);
        }
        let seq = u64::from_be_bytes(frame[8..16].try_into().expect("fixed delta header"));
        if let Some(duplicate) = self.sequence_result(seq)? {
            return Ok(duplicate);
        }
        let text = std::str::from_utf8(&frame[16..]).map_err(|_| ProtocolError::Framing)?;
        let text_chars = text.chars().count();
        if text.is_empty()
            || self.content.len().saturating_add(text.len()) > MAX_CONTENT_BYTES
            || self.content_chars.saturating_add(text_chars) > MAX_CONTENT_CHARS
        {
            return Err(ProtocolError::ContentLimit);
        }
        self.hasher.update(text.as_bytes());
        self.content.push_str(text);
        self.content_chars += text_chars;
        self.commit_sequence(seq);
        Ok(ProviderEvent::Delta {
            seq,
            text: text.to_string(),
        })
    }

    pub(crate) fn accept_text(&mut self, text: &str) -> Result<ProviderEvent, ProtocolError> {
        reject_duplicate_keys(text)?;
        let envelope: Envelope = serde_json::from_str(text).map_err(|_| ProtocolError::Json)?;
        match envelope.message_type.as_str() {
            "run.accepted" => self.accept_accepted(text),
            "tool.call" => self.accept_tool_call(text),
            "response.completed" => self.accept_completed(text),
            "response.failed" => self.accept_failed(text),
            "response.cancelled" => self.accept_cancelled(text),
            _ => Err(ProtocolError::Schema),
        }
    }

    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    pub(crate) fn checkpoint(&self) -> AcknowledgedCheckpoint {
        AcknowledgedCheckpoint {
            seq: self.last_accepted_seq,
            content_sha256: self.content_hash(),
        }
    }

    pub(crate) fn ack<'a>(&'a self, checkpoint: &'a AcknowledgedCheckpoint) -> RunAck<'a> {
        debug_assert_eq!(checkpoint.seq, self.last_accepted_seq);
        RunAck {
            message_type: "run.ack",
            run_id: &self.run_id,
            ack_seq: checkpoint.seq,
            content_sha256: &checkpoint.content_sha256,
        }
    }

    pub(crate) fn last_accepted_seq(&self) -> u64 {
        self.last_accepted_seq
    }

    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    pub(crate) fn resume<'a>(
        &'a self,
        allocation_id: &'a str,
        checkpoint: &'a AcknowledgedCheckpoint,
    ) -> RunResume<'a> {
        debug_assert!(checkpoint.seq <= self.last_accepted_seq);
        RunResume {
            message_type: "run.resume",
            run_id: &self.run_id,
            allocation_id,
            ack_seq: checkpoint.seq,
            content_sha256: &checkpoint.content_sha256,
        }
    }

    fn accept_accepted(&mut self, text: &str) -> Result<ProviderEvent, ProtocolError> {
        let message: RunAccepted = parse(text)?;
        self.validate_run(&message.run_id)?;
        if let Some(duplicate) = self.sequence_result(message.seq)? {
            return Ok(duplicate);
        }
        if message.seq != 1 {
            return Err(ProtocolError::SequenceGap);
        }
        self.commit_sequence(message.seq);
        Ok(ProviderEvent::Accepted { seq: message.seq })
    }

    fn accept_tool_call(&mut self, text: &str) -> Result<ProviderEvent, ProtocolError> {
        self.ensure_accepted()?;
        let message: ToolCall = parse(text)?;
        self.validate_run(&message.run_id)?;
        reject_duplicate_keys(&message.arguments)?;
        let arguments =
            serde_json::from_str::<Value>(&message.arguments).map_err(|_| ProtocolError::Schema)?;
        if !valid_identifier(&message.call_id)
            || !valid_identifier(&message.name)
            || !arguments.is_object()
            || message.arguments.len() > MAX_CONTENT_BYTES
            || !self.content.is_empty()
        {
            return Err(ProtocolError::Schema);
        }
        if let Some(duplicate) = self.sequence_result(message.seq)? {
            return Ok(duplicate);
        }
        self.commit_sequence(message.seq);
        Ok(ProviderEvent::ToolCall {
            seq: message.seq,
            call_id: message.call_id,
            name: message.name,
            arguments,
        })
    }

    fn accept_completed(&mut self, text: &str) -> Result<ProviderEvent, ProtocolError> {
        self.ensure_accepted()?;
        let message: ResponseCompleted = parse(text)?;
        self.validate_run(&message.run_id)?;
        if let Some(duplicate) = self.sequence_result(message.seq)? {
            return Ok(duplicate);
        }
        let finish_reason = match message.finish_reason.as_str() {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::Length,
            _ => return Err(ProtocolError::Schema),
        };
        if message.content_bytes != self.content.len()
            || message.content_sha256 != self.content_hash()
            || message.usage.as_ref().is_some_and(|usage| {
                usage
                    .prompt_tokens
                    .zip(usage.completion_tokens)
                    .zip(usage.total_tokens)
                    .is_some_and(|((prompt, completion), total)| {
                        prompt.checked_add(completion) != Some(total)
                    })
            })
        {
            return Err(ProtocolError::ContentIntegrity);
        }
        self.commit_terminal(message.seq)?;
        Ok(ProviderEvent::Completed {
            seq: message.seq,
            finish_reason,
        })
    }

    fn accept_failed(&mut self, text: &str) -> Result<ProviderEvent, ProtocolError> {
        self.ensure_accepted()?;
        let message: ResponseFailed = parse(text)?;
        self.validate_run(&message.run_id)?;
        if let Some(duplicate) = self.sequence_result(message.seq)? {
            return Ok(duplicate);
        }
        if message.content_bytes != self.content.len()
            || message.content_sha256 != self.content_hash()
            || !valid_error_code(&message.error.code)
            || message.error.message.is_empty()
            || message.error.message.len() > 512
            || message.error.message.chars().any(char::is_control)
        {
            return Err(ProtocolError::ContentIntegrity);
        }
        let _retryable = message.error.retryable;
        self.commit_terminal(message.seq)?;
        Ok(ProviderEvent::Failed {
            seq: message.seq,
            code: message.error.code,
            output_started: message.content_bytes > 0,
        })
    }

    fn accept_cancelled(&mut self, text: &str) -> Result<ProviderEvent, ProtocolError> {
        self.ensure_accepted()?;
        let message: ResponseCancelled = parse(text)?;
        self.validate_run(&message.run_id)?;
        if let Some(duplicate) = self.sequence_result(message.seq)? {
            return Ok(duplicate);
        }
        if message.content_bytes != self.content.len()
            || message.content_sha256 != self.content_hash()
        {
            return Err(ProtocolError::ContentIntegrity);
        }
        self.commit_terminal(message.seq)?;
        Ok(ProviderEvent::Cancelled {
            seq: message.seq,
            output_started: message.content_bytes > 0,
        })
    }

    fn sequence_result(&self, seq: u64) -> Result<Option<ProviderEvent>, ProtocolError> {
        if self.terminal {
            return Err(ProtocolError::Terminal);
        }
        if seq < self.expected_seq {
            return Ok(Some(ProviderEvent::Duplicate { seq }));
        }
        if seq > self.expected_seq || seq == 0 {
            return Err(ProtocolError::SequenceGap);
        }
        Ok(None)
    }

    fn commit_sequence(&mut self, seq: u64) {
        self.last_accepted_seq = seq;
        self.expected_seq = seq.saturating_add(1);
    }

    fn commit_terminal(&mut self, seq: u64) -> Result<(), ProtocolError> {
        self.commit_sequence(seq);
        self.terminal = true;
        Ok(())
    }

    fn validate_run(&self, run_id: &str) -> Result<(), ProtocolError> {
        if run_id == self.run_id {
            Ok(())
        } else {
            Err(ProtocolError::StaleRun)
        }
    }

    fn ensure_accepted(&self) -> Result<(), ProtocolError> {
        (self.last_accepted_seq > 0)
            .then_some(())
            .ok_or(ProtocolError::Schema)
    }

    fn content_hash(&self) -> String {
        let digest = self.hasher.clone().finalize();
        let mut encoded = String::with_capacity(64);
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(encoded, "{byte:02x}");
        }
        encoded
    }
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type")]
    message_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunAccepted {
    #[serde(rename = "type")]
    _message_type: String,
    run_id: String,
    seq: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunResumed {
    #[serde(rename = "type")]
    _message_type: String,
    run_id: String,
    ack_seq: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolCall {
    #[serde(rename = "type")]
    _message_type: String,
    run_id: String,
    seq: u64,
    call_id: String,
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResponseCompleted {
    #[serde(rename = "type")]
    _message_type: String,
    run_id: String,
    seq: u64,
    content_bytes: usize,
    content_sha256: String,
    finish_reason: String,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Usage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResponseFailed {
    #[serde(rename = "type")]
    _message_type: String,
    run_id: String,
    seq: u64,
    content_bytes: usize,
    content_sha256: String,
    error: ResponseError,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResponseError {
    code: String,
    message: String,
    retryable: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResponseCancelled {
    #[serde(rename = "type")]
    _message_type: String,
    run_id: String,
    seq: u64,
    content_bytes: usize,
    content_sha256: String,
}

fn parse<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, ProtocolError> {
    reject_duplicate_keys(text)?;
    serde_json::from_str(text).map_err(|_| ProtocolError::Schema)
}

pub(crate) fn reject_duplicate_keys(text: &str) -> Result<(), ProtocolError> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    NoDuplicateSeed
        .deserialize(&mut deserializer)
        .map_err(|_| ProtocolError::Json)?;
    deserializer.end().map_err(|_| ProtocolError::Json)
}

struct NoDuplicateSeed;

impl<'de> DeserializeSeed<'de> for NoDuplicateSeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(NoDuplicateVisitor)
    }
}

struct NoDuplicateVisitor;

impl<'de> Visitor<'de> for NoDuplicateVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JSON without duplicate object keys")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(A::Error::custom("duplicate JSON key"));
            }
            map.next_value_seed(NoDuplicateSeed)?;
        }
        Ok(())
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence.next_element_seed(NoDuplicateSeed)?.is_some() {}
        Ok(())
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(())
    }
    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(())
    }
    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(())
    }
    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(())
    }
    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(())
    }
    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E> {
        Ok(())
    }
    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }
    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        NoDuplicateSeed.deserialize(deserializer)
    }
    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(())
    }
}

fn valid_identifier(value: &str) -> bool {
    value.len() <= 192
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn valid_error_code(value: &str) -> bool {
    matches!(
        value,
        "invalid-request"
            | "response-too-large"
            | "capacity"
            | "model-unavailable"
            | "provider-error"
            | "provider-timeout"
            | "tool-error"
            | "tool-timeout"
            | "backpressure"
            | "allocation-inactive"
            | "internal-error"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary(seq: u64, text: &str) -> Vec<u8> {
        let mut frame = Vec::with_capacity(16 + text.len());
        frame.extend_from_slice(b"SAD1");
        frame.extend_from_slice(&[1, 0]);
        frame.extend_from_slice(&16_u16.to_be_bytes());
        frame.extend_from_slice(&seq.to_be_bytes());
        frame.extend_from_slice(text.as_bytes());
        frame
    }

    #[test]
    fn run_start_serializes_the_current_strict_contract() {
        let messages = [serde_json::json!({ "role": "user", "content": "hello" })];
        let start = RunStart::new(
            "run_1",
            "alloc_1",
            "coding-default",
            &messages,
            &[],
            "xhigh",
            128,
            5_000,
        );

        assert_eq!(
            serde_json::to_value(start).expect("run.start serializes"),
            serde_json::json!({
                "type": "run.start",
                "runId": "run_1",
                "allocationId": "alloc_1",
                "model": "coding-default",
                "messages": [{ "role": "user", "content": "hello" }],
                "tools": [],
                "reasoning": { "effort": "high" },
                "maxOutputTokens": 128,
                "maxToolCalls": 0,
                "timeoutMs": 5_000
            })
        );
    }

    fn accepted(run: &mut OrderedRun) {
        run.accept_text(r#"{"type":"run.accepted","runId":"run_1","seq":1}"#)
            .expect("accepted");
    }

    #[test]
    fn ten_thousand_deltas_are_ordered_without_loss_or_duplication() {
        let mut run = OrderedRun::new("run_1").expect("run");
        accepted(&mut run);
        for seq in 2..=10_001 {
            let event = run.accept_binary(&binary(seq, "x")).expect("delta");
            assert_eq!(
                event,
                ProviderEvent::Delta {
                    seq,
                    text: "x".to_string()
                }
            );
        }
        assert_eq!(run.content().len(), 10_000);
        assert_eq!(run.last_accepted_seq(), 10_001);
    }

    #[test]
    fn first_provider_event_must_be_run_accepted() {
        let mut run = OrderedRun::new("run_1").expect("run");

        assert_eq!(
            run.accept_binary(&binary(1, "x")),
            Err(ProtocolError::Schema)
        );
        assert_eq!(
            run.accept_text(
                r#"{"type":"response.cancelled","runId":"run_1","seq":1,"outputStarted":false}"#,
            ),
            Err(ProtocolError::Schema)
        );
        assert_eq!(run.last_accepted_seq(), 0);
    }

    #[test]
    fn ten_thousand_twenty_byte_binary_deltas_meet_the_loopback_gate() {
        let mut run = OrderedRun::new("run_1").expect("run");
        accepted(&mut run);
        let delta = "😀😀😀😀日a";
        assert_eq!(delta.len(), 20);
        let started = std::time::Instant::now();
        for seq in 2..=10_001 {
            assert!(matches!(
                run.accept_binary(&binary(seq, delta)),
                Ok(ProviderEvent::Delta { .. })
            ));
        }
        assert!(
            started.elapsed() <= std::time::Duration::from_millis(250),
            "20-byte delta projection took {:?}",
            started.elapsed()
        );
        assert_eq!(run.content().len(), 200_000);
        assert!(run.content.capacity() <= 16 * 1_024 * 1_024);
    }

    #[test]
    fn duplicate_is_ignored_but_gap_and_stale_run_are_rejected() {
        let mut run = OrderedRun::new("run_1").expect("run");
        accepted(&mut run);
        assert!(matches!(
            run.accept_binary(&binary(1, "x")),
            Ok(ProviderEvent::Duplicate { seq: 1 })
        ));
        assert_eq!(
            run.accept_binary(&binary(3, "x")),
            Err(ProtocolError::SequenceGap)
        );
        assert_eq!(
            run.accept_text(
                r#"{"type":"response.cancelled","runId":"run_2","seq":2,"contentBytes":0,"contentSha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}"#
            ),
            Err(ProtocolError::StaleRun),
        );
    }

    #[test]
    fn terminal_checks_exact_incremental_hash_and_byte_count() {
        let mut run = OrderedRun::new("run_1").expect("run");
        accepted(&mut run);
        run.accept_binary(&binary(2, "こんにちは")).expect("delta");
        let hash = run.content_hash();
        let terminal = format!(
            r#"{{"type":"response.completed","runId":"run_1","seq":3,"contentBytes":15,"contentSha256":"{hash}","finishReason":"stop","usage":null}}"#,
        );
        assert_eq!(
            run.accept_text(&terminal),
            Ok(ProviderEvent::Completed {
                seq: 3,
                finish_reason: FinishReason::Stop
            }),
        );
        assert_eq!(
            run.accept_binary(&binary(4, "late")),
            Err(ProtocolError::Terminal)
        );
    }

    #[test]
    fn invalid_terminal_hash_never_completes_the_run() {
        let mut run = OrderedRun::new("run_1").expect("run");
        accepted(&mut run);
        run.accept_binary(&binary(2, "ok")).expect("delta");
        assert_eq!(
            run.accept_text(
                r#"{"type":"response.completed","runId":"run_1","seq":3,"contentBytes":2,"contentSha256":"bad","finishReason":"stop","usage":null}"#,
            ),
            Err(ProtocolError::ContentIntegrity),
        );
    }

    #[test]
    fn duplicate_keys_are_rejected_at_every_json_depth() {
        let mut run = OrderedRun::new("run_1").expect("run");
        assert_eq!(
            run.accept_text(
                r#"{"type":"run.accepted","runId":"run_1","seq":1,"providerRunId":"provider_1","model":"local","client":{"name":"a","name":"b"}}"#,
            ),
            Err(ProtocolError::Json),
        );
        assert_eq!(
            validate_resume_negotiation(
                r#"{"type":"run.resumed","runId":"run_1","runId":"run_1","replayFromSeq":2}"#,
                "run_1",
                2,
            ),
            Err(ProtocolError::Json),
        );
    }

    #[test]
    fn terminal_usage_must_be_arithmetically_consistent() {
        let mut run = OrderedRun::new("run_1").expect("run");
        accepted(&mut run);
        let hash = run.content_hash();
        let terminal = format!(
            r#"{{"type":"response.completed","runId":"run_1","seq":2,"contentBytes":0,"contentSha256":"{hash}","finishReason":"stop","usage":{{"promptTokens":2,"completionTokens":3,"totalTokens":6}}}}"#,
        );
        assert_eq!(
            run.accept_text(&terminal),
            Err(ProtocolError::ContentIntegrity)
        );
    }

    #[test]
    fn resume_negotiation_requires_exact_run_and_replay_sequence() {
        assert_eq!(
            validate_resume_negotiation(
                r#"{"type":"run.resumed","runId":"run_1","ackSeq":42}"#,
                "run_1",
                42,
            ),
            Ok(ResumeNegotiation::Resumed),
        );
        assert_eq!(
            validate_resume_negotiation(
                r#"{"type":"run.resumed","runId":"run_1","ackSeq":41}"#,
                "run_1",
                42,
            ),
            Err(ProtocolError::StaleRun),
        );
    }
}
