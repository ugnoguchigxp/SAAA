//! Validation boundary for the fixed JSONL Codex sidecar protocol.
//!
//! The sidecar is an untrusted child process: a frame is not an answer until its protocol id,
//! step id, size, version, and terminal sequencing have all been checked here.
use serde::Deserialize;

const PROTOCOL_VERSION: u8 = 1;
const MAX_FRAME_BYTES: usize = 1_024 * 1_024;
const MAX_FINAL_BYTES: usize = 64 * 1_024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidecarEvent {
    Started,
    Activity,
    Result {
        text: String,
        usage: Option<SidecarUsage>,
    },
    Failed {
        code: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SidecarUsage {
    pub(crate) input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_output_tokens: u64,
}

impl SidecarUsage {
    pub(crate) fn as_json(&self) -> String {
        serde_json::json!({
            "inputTokens": self.input_tokens,
            "cachedInputTokens": self.cached_input_tokens,
            "outputTokens": self.output_tokens,
            "reasoningOutputTokens": self.reasoning_output_tokens,
        })
        .to_string()
    }

    fn valid(&self) -> bool {
        const MAX_TOKENS: u64 = 100_000_000;
        self.input_tokens <= MAX_TOKENS
            && self.cached_input_tokens <= self.input_tokens
            && self.output_tokens <= MAX_TOKENS
            && self.reasoning_output_tokens <= self.output_tokens
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Frame {
    Started {
        version: u8,
        id: String,
        #[serde(rename = "stepId")]
        step_id: String,
    },
    Activity {
        version: u8,
        id: String,
        #[serde(rename = "stepId")]
        step_id: String,
    },
    Result {
        version: u8,
        id: String,
        #[serde(rename = "stepId")]
        step_id: String,
        text: String,
        #[serde(default)]
        usage: Option<SidecarUsage>,
    },
    Failed {
        version: u8,
        id: String,
        #[serde(rename = "stepId")]
        step_id: String,
        code: String,
    },
    Cancelled {
        version: u8,
        id: String,
        #[serde(rename = "stepId")]
        step_id: String,
    },
}

impl Frame {
    fn header(&self) -> (u8, &str, &str) {
        match self {
            Self::Started {
                version,
                id,
                step_id,
            }
            | Self::Activity {
                version,
                id,
                step_id,
            }
            | Self::Result {
                version,
                id,
                step_id,
                ..
            }
            | Self::Failed {
                version,
                id,
                step_id,
                ..
            }
            | Self::Cancelled {
                version,
                id,
                step_id,
            } => (*version, id, step_id),
        }
    }

    fn event(self) -> SidecarEvent {
        match self {
            Self::Started { .. } => SidecarEvent::Started,
            Self::Activity { .. } => SidecarEvent::Activity,
            Self::Result { text, usage, .. } => SidecarEvent::Result { text, usage },
            Self::Failed { code, .. } => SidecarEvent::Failed { code },
            Self::Cancelled { .. } => SidecarEvent::Cancelled,
        }
    }
}

#[derive(Debug)]
pub(crate) struct FrameValidator {
    id: String,
    step_id: String,
    terminal: bool,
}

impl FrameValidator {
    pub(crate) fn new(id: impl Into<String>, step_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            step_id: step_id.into(),
            terminal: false,
        }
    }

    pub(crate) fn validate(&mut self, line: &[u8]) -> Result<SidecarEvent, String> {
        if line.len() > MAX_FRAME_BYTES {
            return Err("Codex sidecar frame exceeds the protocol limit".into());
        }
        let frame: Frame = serde_json::from_slice(line)
            .map_err(|_| "Codex sidecar frame is invalid".to_string())?;
        let (version, id, step_id) = frame.header();
        if version != PROTOCOL_VERSION || id != self.id || step_id != self.step_id {
            return Err("Codex sidecar frame does not belong to this step".into());
        }
        let event = frame.event();
        if let SidecarEvent::Result { text, usage } = &event {
            if text.is_empty()
                || text.len() > MAX_FINAL_BYTES
                || usage.as_ref().is_some_and(|usage| !usage.valid())
            {
                return Err("Codex sidecar result is invalid".into());
            }
        }
        if matches!(event, SidecarEvent::Failed { ref code } if code.is_empty() || code.len() > 120)
        {
            return Err("Codex sidecar failure code is invalid".into());
        }
        if matches!(
            event,
            SidecarEvent::Result { .. } | SidecarEvent::Failed { .. } | SidecarEvent::Cancelled
        ) {
            if self.terminal {
                return Err("Codex sidecar emitted a duplicate terminal frame".into());
            }
            self.terminal = true;
        }
        Ok(event)
    }

    pub(crate) fn terminal(&self) -> bool {
        self.terminal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rr_19_frame_validation_rejects_unknown_ids_and_duplicate_terminals() {
        let mut validator = FrameValidator::new("request", "step");
        assert!(validator
            .validate(br#"{"version":1,"id":"other","stepId":"step","op":"started"}"#)
            .is_err());
        assert_eq!(
            validator
                .validate(
                    br#"{"version":1,"id":"request","stepId":"step","op":"result","text":"done"}"#
                )
                .expect("result"),
            SidecarEvent::Result {
                text: "done".into(),
                usage: None
            }
        );
        assert!(validator.terminal());
        assert!(validator
            .validate(br#"{"version":1,"id":"request","stepId":"step","op":"cancelled"}"#)
            .is_err());
    }

    #[test]
    fn rr_19_frame_validation_rejects_extra_fields_and_empty_results() {
        let mut validator = FrameValidator::new("request", "step");
        assert!(validator
            .validate(
                br#"{"version":1,"id":"request","stepId":"step","op":"started","extra":true}"#
            )
            .is_err());
        assert!(validator
            .validate(br#"{"version":1,"id":"request","stepId":"step","op":"result","text":""}"#)
            .is_err());
    }

    #[test]
    fn rr_22_usage_is_typed_bounded_and_serialized_without_payload() {
        let mut validator = FrameValidator::new("request", "step");
        let event = validator
            .validate(br#"{"version":1,"id":"request","stepId":"step","op":"result","text":"done","usage":{"inputTokens":8,"cachedInputTokens":3,"outputTokens":5,"reasoningOutputTokens":2}}"#)
            .expect("usage result");
        assert_eq!(
            event,
            SidecarEvent::Result {
                text: "done".into(),
                usage: Some(SidecarUsage {
                    input_tokens: 8,
                    cached_input_tokens: 3,
                    output_tokens: 5,
                    reasoning_output_tokens: 2,
                }),
            }
        );
        let mut invalid = FrameValidator::new("request", "step");
        assert!(invalid
            .validate(br#"{"version":1,"id":"request","stepId":"step","op":"result","text":"done","usage":{"inputTokens":3,"cachedInputTokens":4,"outputTokens":5,"reasoningOutputTokens":2}}"#)
            .is_err());
    }
}
