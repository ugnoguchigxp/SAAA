#![cfg(test)]
//! G1 integration tests: the fixed graph question reaches one authorized five-element slice and the
//! C3/C5 boundaries stay explicit. Deterministic fixtures only; no model, network or production DB.

use super::question::{parse_graph_question, QuestionParse};
use super::render::parse_rendered_json;
use super::source::{
    prepare_candidate, prepare_explicit_question_candidate, WorldOmission, WorldSourceOutcome,
    WorldSourceRequest, WORLD_KIND,
};
use super::turn::{compose_parts, TurnCompose};
use crate::memory::context_window::{ContextHealthReport, ContextWindow, ProjectedContextMessage};
use crate::memory::personal_state::world::query::WorldSeed;
use crate::memory::personal_state::world::query_v2::IncludeFlags;
use crate::memory::personal_state::world::runtime_frame::GraphRequest;
use crate::memory::personal_state::world::runtime_test_support::{Fixture, RUN_ID};
use crate::memory::personal_state::world::test_support::*;
use crate::runtime::context::scope::ScopeSnapshot;
use saaa_personal_state_core::world::model_v2::EntityKindV2;
use saaa_personal_state_core::world::traversal_v2::{CausalDirection, LimitsV2};
use saaa_personal_state_core::SourceRef;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;
include!("g1_tests.d/01.rs");
include!("g1_tests.d/02.rs");
