use super::{
    contracts::{GoalProposal, PlanStep, TaskPlan, Verifier, MAX_ACTIVE_GOALS},
    CONTINUE_TRIGGER, DEDUPE_SUFFIX, START_REQUEST, START_TRIGGER,
};
use crate::{database_error, new_id, now_iso, AppState};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
#[path = "repository/active_work.rs"]
mod active_work;
#[path = "repository/queue_task.rs"]
mod queue_task;
#[path = "repository/terminal_report.rs"]
mod terminal_report;
#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) use active_work::{active_delegation, register, withdraw};
pub(crate) use active_work::{
    active_delegations, active_goal_job_ids, active_goal_work, forget_source, latest_work, list,
    propose, register_with_options, withdraw_goal, ActiveWork,
};
pub(crate) use queue_task::{
    active_job_ids, apply_terminal_event, budget_exceeded, claim_dispatch, inspectable_jobs,
    persist_task_plan, queue_task, reorder_queue, replan_after_failure, set_loop_state,
    settle_dispatch, task_step_recipe,
};
#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) use queue_task::{completed_recipe, next_queued_work, queued_task};
pub(crate) use terminal_report::delegated_profile_available;
pub(crate) use terminal_report::{
    claim_pending_speech, claim_terminals, coding_enabled, enqueue_task_report, input_message_id,
    last_foreground, mark_flushed, mark_speech_state, store_foreground, suppress_pending_speech,
    sync_from_coding, triggers, unflushed_digest, workspace_registered, TerminalReport,
};
#[cfg(any(test, feature = "offline-contracts"))]
pub(crate) use terminal_report::{
    enqueue_report, map_coding_state, request_forbidden, start_request, PendingSpeech,
};
