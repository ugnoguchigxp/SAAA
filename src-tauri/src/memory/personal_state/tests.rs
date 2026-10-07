#![cfg(test)]
use super::*;
use super::{projection::snapshots, sources::episode_export, worker::admission};
use crate::persistence::sqlite::SqliteWriter;
use rusqlite::{params, Connection};
use serde_json::json;
#[path = "tests/fixture.rs"]
mod fixture;
use fixture::*;
#[path = "tests/task_bundle_rolls_back_with_answer_and_rejects_e.rs"]
mod task_bundle_rolls_back_with_answer_and_rejects_e;

#[path = "tests/memory_contract.rs"]
mod memory_contract;

#[path = "tests/artifact_invalidation.rs"]
mod artifact_invalidation;

#[path = "tests/consolidation.rs"]
mod consolidation;

#[path = "tests/source_clock.rs"]
mod source_clock;

#[path = "tests/episode_history.rs"]
mod episode_history;
