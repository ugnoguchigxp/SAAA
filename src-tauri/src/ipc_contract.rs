use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub(crate) use crate::voice_behavior::{
    ConversationVoicePolicySnapshot, VoicePresentationDecision,
};

mod bindings;
pub use bindings::{typescript_bindings, ui_typescript_bindings};

macro_rules! runtime_failure_codes {
    ($( $variant:ident => $wire_value:literal ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, Deserialize, Serialize, TS)]
        pub(crate) enum RuntimeFailureCode {
            $(
                #[serde(rename = $wire_value)]
                $variant,
            )+
        }

        impl RuntimeFailureCode {
            const ALL: &'static [Self] = &[$(Self::$variant),+];
        }
    };
}

runtime_failure_codes! {
    RuntimeError => "runtime_error",
    ConfigurationError => "configuration-error",
    ChildStartFailed => "child-start-failed",
    RequestTimeout => "request-timeout",
    ProgressTimeout => "progress-timeout",
    TerminalTimeout => "terminal-timeout",
    HardTimeout => "hard-timeout",
    ChildExited => "child-exited",
    ProtocolError => "protocol-error",
    PolicyViolation => "policy-violation",
    ProviderError => "provider-error",
    ResponseTooLarge => "response-too-large",
    RequiredContextOverflow => "required-context-overflow",
    ContextScopeChanged => "context-scope-changed",
    RequiredContextUnavailable => "required-context-unavailable",
    InternalError => "internal-error",
}

mod conversation_message;
pub(crate) use conversation_message::ConversationMessage;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationMessagePage {
    pub(crate) messages: Vec<ConversationMessage>,
    pub(crate) has_more: bool,
    pub(crate) next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum RuntimeEvent {
    Started {
        run_id: String,
        route: String,
        provider_id: String,
    },
    Delta {
        run_id: String,
        text: String,
    },
    Activity {
        run_id: String,
        kind: String,
        summary: String,
    },
    ProviderFailed {
        run_id: String,
        provider_id: String,
        reason: String,
    },
    MessageCompleted {
        run_id: String,
        message: ConversationMessage,
        presentation: VoicePresentationDecision,
        voice_policy: Option<Box<ConversationVoicePolicySnapshot>>,
    },
    SpeechStarted {
        run_id: String,
    },
    SpeechEnded {
        run_id: String,
    },
    SpeechFailed {
        run_id: String,
        message: String,
        recovery: String,
    },
    Cancelled {
        run_id: String,
    },
    Failed {
        run_id: String,
        code: RuntimeFailureCode,
        message: String,
        recovery: String,
    },
}

pub use crate::artifact_preview::typescript_bindings as artifact_preview_typescript_bindings;
pub use crate::coding::contracts::typescript_bindings as coding_typescript_bindings;
pub use crate::schedule::contracts::typescript_bindings as schedule_typescript_bindings;

pub fn steward_typescript_bindings() -> String {
    crate::steward::execution_contracts::typescript_bindings()
}
