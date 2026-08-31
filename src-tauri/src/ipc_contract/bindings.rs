use ts_rs::{Config, TS};

use super::{
    ConversationMessage, ConversationVoicePolicySnapshot, RuntimeEvent, RuntimeFailureCode,
    VoicePresentationDecision,
};

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
         {}\n\n\
         {}\n\n\
         {}\n",
        export_declaration::<ConversationMessage>(),
        export_declaration::<VoicePresentationDecision>(),
        export_declaration::<ConversationVoicePolicySnapshot>(),
        export_declaration::<RuntimeEvent>(),
    )
}
