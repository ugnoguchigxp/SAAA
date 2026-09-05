use serde::{Deserialize, Serialize};
use ts_rs::{Config, TS};

#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartVoiceAsrSessionInput {
    pub(crate) session_id: String,
    pub(crate) conversation_id: String,
    pub(crate) sample_rate: u32,
    #[serde(default)]
    #[ts(optional)]
    pub(crate) recover_existing: Option<bool>,
}
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CommitVoiceAsrUtteranceInput {
    pub(crate) session_id: String,
    pub(crate) reason: CommitReason,
}
#[derive(Debug, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StopVoiceAsrSessionInput {
    pub(crate) session_id: String,
    pub(crate) finalize_current: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CommitReason {
    Silence,
    MaxDuration,
}

macro_rules! voice_asr_failure_codes {
    ($( $variant:ident => $wire_value:literal ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
        pub(crate) enum VoiceAsrFailureCode {
            $(
                #[serde(rename = $wire_value)]
                $variant,
            )+
        }

        impl VoiceAsrFailureCode {
            #[cfg(test)]
            const ALL: &'static [Self] = &[$(Self::$variant),+];
        }
    };
}

voice_asr_failure_codes! {
    SessionExists => "asr-session-exists",
    SessionNotFound => "asr-session-not-found",
    PacketFormat => "asr-packet-format",
    PacketSequence => "asr-packet-sequence",
    Backpressure => "asr-backpressure",
    ProviderUnavailable => "asr-provider-unavailable",
    StreamProtocol => "asr-stream-protocol",
    StreamTimeout => "asr-stream-timeout",
    FinalTimeout => "asr-final-timeout",
    TargetSpeakerUnavailable => "asr-target-speaker-unavailable",
    LanguageNotAllowed => "asr-language-not-allowed",
    NoSpeech => "asr-no-speech",
    Cancelled => "asr-cancelled",
}

#[allow(
    dead_code,
    reason = "fields are consumed through serialized IPC events"
)]
#[derive(Debug, Clone, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum VoiceAsrStreamEvent {
    Ready {
        session_id: String,
        current_utterance_id: String,
        #[ts(type = "\"native\" | \"batch-agreement\"")]
        protocol: &'static str,
        #[ts(type = "\"all-speakers\" | \"target-speaker\"")]
        scope: &'static str,
    },
    Partial {
        session_id: String,
        utterance_id: String,
        #[ts(type = "number")]
        revision: u64,
        #[ts(type = "number")]
        start_ms: u64,
        #[ts(type = "number")]
        end_ms: u64,
        stable_text: String,
        unstable_text: String,
        language: Option<String>,
    },
    UtteranceDiscarded {
        session_id: String,
        utterance_id: String,
        #[ts(type = "\"no-speech\" | \"target-speaker-empty\" | \"cancelled\"")]
        reason: &'static str,
    },
    Final {
        session_id: String,
        utterance_id: String,
        #[ts(type = "number")]
        revision: u64,
        #[ts(type = "number")]
        start_ms: u64,
        #[ts(type = "number")]
        end_ms: u64,
        text: String,
        language: Option<String>,
    },
    Failed {
        session_id: String,
        utterance_id: Option<String>,
        code: VoiceAsrFailureCode,
        message: String,
        recovery: String,
        fatal: bool,
    },
    Degraded {
        session_id: String,
        #[ts(type = "\"native\"")]
        from: &'static str,
        #[ts(type = "\"batch-agreement\"")]
        to: &'static str,
        reason_code: VoiceAsrFailureCode,
    },
    Stopped {
        session_id: String,
    },
}

fn export_declaration<T: TS>() -> String {
    format!("export {}", T::decl(&Config::default()))
}

pub(crate) fn typescript_bindings() -> String {
    format!(
        "// Generated from src-tauri/src/voice/streaming_asr/contracts.rs. Do not edit by hand.\n\
         // Run `bun run ipc:generate` after changing the Rust voice ASR contract.\n\n\
         {}\n\n\
         {}\n\n\
         {}\n\n\
         {}\n\n\
         {}\n\n\
         {}\n",
        export_declaration::<VoiceAsrFailureCode>(),
        export_declaration::<VoiceAsrStreamEvent>(),
        export_declaration::<CommitReason>(),
        export_declaration::<StartVoiceAsrSessionInput>(),
        export_declaration::<CommitVoiceAsrUtteranceInput>(),
        export_declaration::<StopVoiceAsrSessionInput>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_uses_the_generated_camel_case_wire_shape() {
        let events = [
            VoiceAsrStreamEvent::Ready {
                session_id: "s".into(),
                current_utterance_id: "u".into(),
                protocol: "native",
                scope: "all-speakers",
            },
            VoiceAsrStreamEvent::Partial {
                session_id: "s".into(),
                utterance_id: "u".into(),
                revision: 1,
                start_ms: 0,
                end_ms: 100,
                stable_text: "a".into(),
                unstable_text: "b".into(),
                language: Some("en".into()),
            },
            VoiceAsrStreamEvent::UtteranceDiscarded {
                session_id: "s".into(),
                utterance_id: "u".into(),
                reason: "no-speech",
            },
            VoiceAsrStreamEvent::Final {
                session_id: "s".into(),
                utterance_id: "u".into(),
                revision: 2,
                start_ms: 0,
                end_ms: 100,
                text: "ab".into(),
                language: Some("en".into()),
            },
            VoiceAsrStreamEvent::Failed {
                session_id: "s".into(),
                utterance_id: Some("u".into()),
                code: VoiceAsrFailureCode::StreamTimeout,
                message: "failed".into(),
                recovery: "retry".into(),
                fatal: false,
            },
            VoiceAsrStreamEvent::Degraded {
                session_id: "s".into(),
                from: "native",
                to: "batch-agreement",
                reason_code: VoiceAsrFailureCode::StreamTimeout,
            },
            VoiceAsrStreamEvent::Stopped {
                session_id: "s".into(),
            },
        ];
        for event in events {
            let value = serde_json::to_value(event).unwrap();
            assert!(value.get("sessionId").is_some());
            assert!(value.get("session_id").is_none());
        }
    }

    #[test]
    fn generated_failure_union_contains_every_runtime_wire_value() {
        let binding = typescript_bindings();
        for code in VoiceAsrFailureCode::ALL {
            let code = serde_json::to_string(code).unwrap();
            assert!(binding.contains(&code), "missing {code}");
        }
    }
}
