use rusqlite::{params, OptionalExtension};
use std::fs;
use std::sync::Arc;
#[path = "start_turn.rs"]
pub(crate) mod command;
use super::event_hub::RuntimeEventSender;
use crate::ipc_contract::{RuntimeEvent, RuntimeFailureCode};
use crate::persistence::conversations::validate_conversation_write_target;
use crate::redact::{bounded_text, redact_runtime_text};
use crate::{
    database_error, execute_codex_turn, memory, new_id, now_iso, situation, AppState,
    RunCancellation, StartTurnInput, TurnExecutionFailure,
};
include!("turns.d/01.rs");
include!("turns.d/02.rs");
