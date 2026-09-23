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
pub(crate) use active_work::{ActiveWork, register, register_with_options, propose, withdraw, list, withdraw_goal, forget_source, active_goal_job_ids, latest_work, active_delegation, active_delegations, active_goal_work};
pub(crate) use queue_task::{queue_task, claim_dispatch, settle_dispatch, apply_terminal_event, replan_after_failure, set_loop_state, persist_task_plan, completed_recipe, task_step_recipe, queued_task, next_queued_work, reorder_queue, inspectable_jobs, active_job_ids, budget_exceeded};
pub(crate) use terminal_report::{workspace_registered, input_message_id, last_foreground, store_foreground, map_coding_state, sync_from_coding, TerminalReport, claim_terminals, enqueue_task_report, enqueue_report, unflushed_digest, mark_flushed, PendingSpeech, claim_pending_speech, mark_speech_state, suppress_pending_speech, start_request, triggers, request_forbidden, coding_enabled};
pub(crate) use terminal_report::{delegated_profile_available};
