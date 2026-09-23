//! Multi-message conversation continuation.
//! Set `SAAA_BUTLER_CONTINUATION=0` to keep the previous one-answer path.
#![allow(dead_code)]

mod action;
pub(crate) mod command;
mod ledger;

pub use action::{from_provider_turn, AgentAction, ProviderFinish, ProviderTurn, ToolCall};
pub use ledger::{
    accept_run_input, append_event, begin_work, commit_preface, commit_visible_message,
    ensure_schema, finish_input_round, finish_work, load_work_state, mark_inputs_consumed,
    mark_presentation_unconfirmed, message_was_presented, peek_pending_user_texts,
    record_tool_event, record_work_reference, update_work_state, WorkState, LEDGER_DDL,
};

pub fn continuation_enabled() -> bool {
    std::env::var("SAAA_BUTLER_CONTINUATION").ok().as_deref() != Some("0")
}

#[cfg(test)]
mod tests;
