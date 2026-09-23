#![cfg(test)]

//! WM adapter integration tests (WM-02..WM-15). Deterministic fixtures only.

use super::query::{activate, ActivateInput, WorldSeed};
use super::test_support::*;
use crate::memory::personal_state::{encode, now, sources, store};
use saaa_personal_state_core::world::traversal::{CausalDirection, Limits, TraversalMode};
use saaa_personal_state_core::world::*;
use saaa_personal_state_core::*;
use serde_json::json;
#[path = "tests/fixture.rs"]
mod fixture;
use fixture::*;
#[path = "tests/t14_scope_isolation_and_authorization.rs"]
mod t14_scope_isolation_and_authorization;
#[path = "tests/t11_patch_replay_is_noop_and_content_conflict_is.rs"]
mod t11_patch_replay_is_noop_and_content_conflict_is;
pub(super) use t11_patch_replay_is_noop_and_content_conflict_is::query_at;
#[path = "tests/r06_unrelated_focus_does_not_join_another_focus_.rs"]
mod r06_unrelated_focus_does_not_join_another_focus_;
