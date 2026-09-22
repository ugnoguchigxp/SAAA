mod app_state;
use app_state::{AppState, ProviderProbeStatus, RunCancellation};
#[path = "providers/larm_voice/mod.rs"]
mod larm_voice;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::Manager;
mod adaptive_evaluation;
mod adaptive_improvement;
mod app_paths;
mod artifact_preview;
mod backup;
mod coding;
#[path = "runtime/command_registry.rs"]
mod command_registry;
mod credentials;
mod database_backup;
mod diagnosis;
mod diagnostics;
pub mod generated_capabilities;
mod generative_ui;
#[cfg(feature = "provider-diagnostics")]
pub mod harness_llm_diagnostic;
pub mod ipc_contract;
mod memory;
mod models;
mod persistence;
mod process_guard;
mod providers;
#[cfg(feature = "quality-eval-harness")]
pub mod quality_eval;
mod redact;
mod role_routing;
pub mod runtime;
mod schedule;
mod situation;
mod steward;
#[cfg(test)]
mod test_state;
#[cfg(test)]
mod test_support;
pub mod tool_selection;
mod util;
mod voice;
pub mod voice_asr_contract;
mod voice_behavior;
mod voice_commands;
mod voice_text;
#[cfg(test)]
mod wasm_host_poc;
mod window_size;
use ipc_contract::ConversationMessagePage;
pub(crate) use models::*;
use persistence::{list_message_page_from_connection, SqliteReaders, SqliteWriter};
pub(crate) use providers::session_store::{
    begin_provider_session, finish_dynamic_lan_provider_session, finish_provider_session,
    persist_conversation_success, persist_conversation_success_with_state,
};
pub(crate) use providers::stream::*;
pub(crate) use redact::{bounded_text, redact_runtime_text};
pub(crate) use runtime::codex_cli::*;
pub(crate) use runtime::codex_turn::execute_codex_turn;
pub(crate) use runtime::run_support::*;
pub(crate) use runtime::turn_types::*;
pub(crate) use runtime::turns::{execute_turn, send_runtime_terminal_event};
pub(crate) use situation::spawn_situation_monitor;
pub(crate) use util::{database_error, new_id, now_iso, validate_identifier};
use voice::streaming_asr::{
    append_voice_asr_audio, commit_voice_asr_utterance, start_voice_asr_session,
    stop_voice_asr_session, AsrSessionManager,
};
use voice_behavior::{
    get_conversation_voice_policy, reset_conversation_voice_policy,
    update_conversation_voice_policy,
};
use voice_commands::{
    delete_voice_enrollment_sample, delete_voice_profile, get_voice_profile_snapshot,
    read_voice_enrollment_sample, save_voice_enrollment_sample, set_target_speaker_filter_enabled,
    stage_audio_upload, stop_tts,
};
mod ipc_receiver_tests;
mod test_environment;
mod tests;
include!("lib.d/01.rs");
include!("lib.d/02.rs");
