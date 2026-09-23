#![cfg(test)]
//! G1 integration tests: the fixed graph question reaches one authorized five-element slice and the
//! C3/C5 boundaries stay explicit. Deterministic fixtures only; no model, network or production DB.

pub(super) use super::question::{parse_graph_question, QuestionParse};
pub(super) use super::render::parse_rendered_json;
pub(super) use super::source::{
    prepare_candidate, prepare_explicit_question_candidate, WorldOmission, WorldSourceOutcome,
    WorldSourceRequest, WORLD_KIND,
};
pub(super) use super::turn::{compose_parts, TurnCompose};
pub(super) use crate::memory::context_window::{
    ContextHealthReport, ContextWindow, ProjectedContextMessage,
};
pub(super) use crate::memory::personal_state::world::query::WorldSeed;
pub(super) use crate::memory::personal_state::world::query_v2::IncludeFlags;
pub(super) use crate::memory::personal_state::world::runtime_frame::GraphRequest;
pub(super) use crate::memory::personal_state::world::runtime_test_support::{Fixture, RUN_ID};
pub(super) use crate::memory::personal_state::world::test_support::*;
pub(super) use crate::runtime::context::scope::ScopeSnapshot;
pub(super) use saaa_personal_state_core::world::model_v2::EntityKindV2;
pub(super) use saaa_personal_state_core::world::traversal_v2::{CausalDirection, LimitsV2};
pub(super) use saaa_personal_state_core::SourceRef;
pub(super) use serde_json::Value;
pub(super) use std::collections::BTreeSet;
pub(super) use std::sync::Arc;
#[path = "g1_tests/tech_question.rs"]
mod tech_question;
#[path = "g1_tests/world_g1_04_graph_only_question_fetches_a_frame_.rs"]
mod world_g1_04_graph_only_question_fetches_a_frame_;

pub(crate) use tech_question::*;
