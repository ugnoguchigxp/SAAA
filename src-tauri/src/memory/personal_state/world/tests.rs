#![cfg(test)]

//! WM adapter integration tests (WM-02..WM-15). Deterministic fixtures only.

use super::query::{activate, ActivateInput, WorldSeed};
use super::test_support::*;
use crate::memory::personal_state::{encode, now, sources, store};
use saaa_personal_state_core::world::traversal::{CausalDirection, Limits, TraversalMode};
use saaa_personal_state_core::world::*;
use saaa_personal_state_core::*;
use serde_json::json;
include!("tests.d/01.rs");
include!("tests.d/02.rs");
include!("tests.d/03.rs");
include!("tests.d/04.rs");
