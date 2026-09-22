use super::contracts::{GoalProposal, Notify, Operation, PlanStep, TaskPlan, Verifier};use super::repository as repo;use super::{CONTINUE_TRIGGER, START_REQUEST, START_TRIGGER};use crate::persistence::schema::{initialize_database, DATABASE_SCHEMA_VERSION};use crate::runtime::turns::prepare_runtime_run;use crate::situation::contracts::ForegroundCategory;use crate::situation::speech_holds_tts;use crate::test_support::app_state;use crate::{AppState, StartTurnInput, PRIMARY_CONVERSATION_ID};use rusqlite::{params, Connection};use std::sync::{Mutex, OnceLock};
include!("tests.d/01.rs");
include!("tests.d/02.rs");
include!("tests.d/03.rs");
