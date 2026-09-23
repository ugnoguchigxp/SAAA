#![cfg(test)]

//! v2 adapter integration tests (D21/D23/D31/D32/D38/D39). Deterministic
//! fixtures only; no model, network or production DB.

pub(crate) use super::outcome_v2::{commit_prepared_outcome, prepare_outcome_patch};
pub(crate) use super::query::WorldSeed;
pub(crate) use super::query_v2::{
    activate_v2, activate_v2_with_stats, ActivateInputV2, IncludeFlags,
};
pub(crate) use super::test_support::*;
pub(crate) use crate::memory::personal_state::{now, store};
pub(crate) use crate::persistence::sqlite::SqliteWriter;
pub(crate) use saaa_personal_state_core::world::model_v2::EffectDirection;
pub(crate) use saaa_personal_state_core::world::model_v2::EntityKindV2;
pub(crate) use saaa_personal_state_core::world::outcome_v2::{Outcome, Prediction};
pub(crate) use saaa_personal_state_core::world::slice_v2::WorldSliceV2;
pub(crate) use saaa_personal_state_core::world::traversal_v2::{CausalDirection, LimitsV2};
pub(crate) use saaa_personal_state_core::*;
pub(crate) use serde_json::json;
pub(crate) use std::collections::BTreeSet;
#[path = "v2_tests/v2_fixture.rs"]
mod v2_fixture;
pub(crate) use v2_fixture::*;
#[path = "v2_tests/chain_fixture.rs"]
mod chain_fixture;
pub(crate) use chain_fixture::*;
#[path = "v2_tests/d19_failed_commit_rolls_back_every_write.rs"]
mod d19_failed_commit_rolls_back_every_write;
#[path = "v2_tests/fixture_chain_v2.rs"]
mod fixture_chain_v2;
