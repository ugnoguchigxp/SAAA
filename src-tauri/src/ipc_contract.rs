use serde::Serialize;
use ts_rs::{Config, TS};

macro_rules! runtime_failure_codes {
    ($( $variant:ident => $wire_value:literal ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, Serialize, TS)]
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

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConversationMessage {
    pub(crate) id: String,
    pub(crate) conversation_id: String,
    #[ts(type = "\"user\" | \"assistant\" | \"system\" | \"transcript\"")]
    pub(crate) role: String,
    pub(crate) content: String,
    pub(crate) created_at: String,
}

#[derive(Debug, Clone, Serialize, TS)]
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

fn export_declaration<T: TS>() -> String {
    format!("export {}", T::decl(&Config::default()))
}

pub fn typescript_bindings() -> String {
    let failure_codes = RuntimeFailureCode::ALL
        .iter()
        .map(|code| serde_json::to_string(code).expect("runtime failure code serializes"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "// Generated from src-tauri/src/ipc_contract.rs. Do not edit by hand.\n\
         // Run `bun run ipc:generate` after changing the Rust IPC contract.\n\n\
         {}\n\n\
         export const runtimeFailureCodes = [{failure_codes}] as const;\n\
         export type RuntimeFailureCode = (typeof runtimeFailureCodes)[number];\n\n\
         {}\n",
        export_declaration::<ConversationMessage>(),
        export_declaration::<RuntimeEvent>(),
    )
}
