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
pub(crate) use active_work::{
    active_delegation, active_delegations, active_goal_job_ids, active_goal_work, forget_source,
    latest_work, list, propose, register, register_with_options, withdraw, withdraw_goal,
    ActiveWork,
};
pub(crate) use queue_task::{
    active_job_ids, apply_terminal_event, budget_exceeded, claim_dispatch, completed_recipe,
    inspectable_jobs, next_queued_work, persist_task_plan, queue_task, queued_task, reorder_queue,
    replan_after_failure, set_loop_state, settle_dispatch, task_step_recipe,
};
pub(crate) use terminal_report::delegated_profile_available;
pub(crate) use terminal_report::{
    claim_pending_speech, claim_terminals, coding_enabled, enqueue_report, enqueue_task_report,
    input_message_id, last_foreground, map_coding_state, mark_flushed, mark_speech_state,
    request_forbidden, start_request, store_foreground, suppress_pending_speech, sync_from_coding,
    triggers, unflushed_digest, workspace_registered, PendingSpeech, TerminalReport,
};
