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
#[path = "turns/execute_turn.rs"]
mod execute_turn;
#[path = "turns/prepare_runtime_run.rs"]
mod prepare_runtime_run;
use execute_turn::public_failure_code;
pub(crate) use execute_turn::{
    execute_turn, expire_stale_input_barrier, finish_supervised_runtime_run,
    send_runtime_terminal_event,
};
#[cfg(test)]
pub(crate) use prepare_runtime_run::finish_runtime_run;
pub(crate) use prepare_runtime_run::prepare_runtime_run;
