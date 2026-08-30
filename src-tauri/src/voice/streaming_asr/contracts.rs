use serde::{Deserialize, Serialize};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartVoiceAsrSessionInput {
    pub(crate) session_id: String,
    pub(crate) conversation_id: String,
    pub(crate) sample_rate: u32,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CommitVoiceAsrUtteranceInput {
    pub(crate) session_id: String,
    pub(crate) reason: CommitReason,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StopVoiceAsrSessionInput {
    pub(crate) session_id: String,
    pub(crate) finalize_current: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CommitReason {
    Silence,
    MaxDuration,
}
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum VoiceAsrStreamEvent {
    Ready {
        session_id: String,
        current_utterance_id: String,
        protocol: &'static str,
        scope: &'static str,
    },
    Partial {
        session_id: String,
        utterance_id: String,
        revision: u64,
        start_ms: u64,
        end_ms: u64,
        stable_text: String,
        unstable_text: String,
        language: Option<String>,
    },
    UtteranceDiscarded {
        session_id: String,
        utterance_id: String,
        reason: &'static str,
    },
    Final {
        session_id: String,
        utterance_id: String,
        revision: u64,
        start_ms: u64,
        end_ms: u64,
        text: String,
        language: Option<String>,
    },
    Failed {
        session_id: String,
        utterance_id: Option<String>,
        code: &'static str,
        message: String,
        recovery: String,
        fatal: bool,
    },
    Degraded {
        session_id: String,
        from: &'static str,
        to: &'static str,
        reason_code: &'static str,
    },
    Stopped {
        session_id: String,
    },
}
