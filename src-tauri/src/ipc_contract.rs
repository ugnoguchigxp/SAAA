use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub(crate) use crate::voice_behavior::{
    ConversationVoicePolicySnapshot, VoicePresentationDecision,
};

mod bindings;
pub use bindings::typescript_bindings;
mod websocket_state;
pub(crate) use websocket_state::WebSocketConnectionState;

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
    InternalError => "internal-error",
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationMessage {
    pub(crate) id: String,
    pub(crate) conversation_id: String,
    #[ts(type = "\"user\" | \"assistant\" | \"system\" | \"transcript\"")]
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) created_at: String,
}

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
    ProviderSelected {
        run_id: String,
        provider_id: String,
        #[ts(type = "\"larm\"")]
        provider_kind: String,
        #[ts(type = "\"llm-default\"")]
        route_id: String,
        runtime_id: String,
        fallback_used: bool,
        #[ts(type = "\"primary\" | \"other\"")]
        selection_reason_code: String,
    },
    WebSocketStateChanged {
        run_id: String,
        #[ts(type = "\"connected\" | \"connecting\" | \"disconnected\"")]
        state: WebSocketConnectionState,
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
