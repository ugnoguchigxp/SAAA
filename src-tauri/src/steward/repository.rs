use super::{
    contracts::{GoalProposal, PlanStep, TaskPlan, Verifier, MAX_ACTIVE_GOALS},
    CONTINUE_TRIGGER, DEDUPE_SUFFIX, START_REQUEST, START_TRIGGER,
};
use crate::{database_error, new_id, now_iso, AppState};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

include!("repository.d/01.rs");
include!("repository.d/02.rs");
include!("repository.d/03.rs");
