#![cfg(test)]

//! v2 adapter integration tests (D21/D23/D31/D32/D38/D39). Deterministic
//! fixtures only; no model, network or production DB.

use super::outcome_v2::{commit_prepared_outcome, prepare_outcome_patch};
use super::query::WorldSeed;
use super::query_v2::{activate_v2, activate_v2_with_stats, ActivateInputV2, IncludeFlags};
use super::test_support::*;
use crate::memory::personal_state::{now, store};
use crate::persistence::sqlite::SqliteWriter;
use saaa_personal_state_core::world::model_v2::EffectDirection;
use saaa_personal_state_core::world::model_v2::EntityKindV2;
use saaa_personal_state_core::world::outcome_v2::{Outcome, Prediction};
use saaa_personal_state_core::world::slice_v2::WorldSliceV2;
use saaa_personal_state_core::world::traversal_v2::{CausalDirection, LimitsV2};
use saaa_personal_state_core::*;
use serde_json::json;
use std::collections::BTreeSet;
include!("v2_tests.d/01.rs");
include!("v2_tests.d/02.rs");
include!("v2_tests.d/03.rs");
include!("v2_tests.d/04.rs");
