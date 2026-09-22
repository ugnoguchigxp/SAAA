//! Internal v2 World query (D21/D23/D31/D32/C7/C8).
//!
//! Read-only. A stale or missing projection is an omission notice, never a
//! repair. Authorization failures are contract errors. Every projected row is
//! re-authorized and re-checked for current validity at read time.

#![allow(clippy::too_many_arguments)]

use super::observations_v2::{
    validate_observations, AvailabilityObservationInput, ConditionObservationInput,
    ValidatedObservations,
};
use super::query::{WorldSeed, AMBIGUOUS_SEED, PENDING, STALE, UNKNOWN_SEED};
use crate::database_error;
use crate::memory::personal_state::store;
use rusqlite::{named_params, Connection};
use saaa_personal_state_core::world::conditions_v2::{evaluate_availability, evaluate_conditions};
use saaa_personal_state_core::world::model_v2::{EffectDirection, EntityKindV2, RelationTypeV2};
use saaa_personal_state_core::world::relevance_v2::{
    build_gap_candidates, dependencies, focus_rank_v2, order_focus, temporary_attention_focus,
    FocusCandidate, GapInputV2, GapRelationV2, OutcomeConflictV2, UnknownAvailabilityV2,
};
use saaa_personal_state_core::world::slice_v2::*;
use saaa_personal_state_core::world::traversal_v2::{
    evaluate_path, maximal_only_v2, traverse_v2, CausalDirection, LimitsV2, TraversalModeV2,
    WorldEdgeV2, WorldPathV2,
};
use saaa_personal_state_core::world::versioned::{decode_versioned, WorldView};
use saaa_personal_state_core::{AccessRequest, Classification, Ledger, Purpose};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IncludeFlags {
    pub causal: bool,
    pub goals: bool,
    pub correlations: bool,
    pub dependencies: bool,
}

impl Default for IncludeFlags {
    fn default() -> Self {
        Self {
            causal: true,
            goals: true,
            correlations: true,
            dependencies: true,
        }
    }
}

pub struct ActivateInputV2<'a> {
    pub project_scope: &'a str,
    pub access: &'a AccessRequest<'a>,
    pub now: i64,
    pub seeds: &'a [WorldSeed],
    pub causal_direction: CausalDirection,
    pub limits: LimitsV2,
    pub max_bytes: usize,
    pub request_id: &'a str,
    pub explicit_question: bool,
    pub flags: IncludeFlags,
    pub condition_observations: &'a [ConditionObservationInput],
    pub availability_observations: &'a [AvailabilityObservationInput],
    pub temporary_attention_entity_ids: &'a [String],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueryStatsV2 {
    pub fetch_rows: usize,
    pub scan_steps: usize,
}

#[derive(Debug, Clone)]
pub struct EntityV2Row {
    pub assertion_id: String,
    pub entity_id: String,
    pub kind: EntityKindV2,
    pub name: String,
    pub objective_assertion_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FocusRowV2 {
    pub assertion_id: String,
    pub entity_id: String,
    pub reason: String,
    pub objective_assertion_id: Option<String>,
}

#[derive(Debug, Default)]
pub struct QueryContextV2 {
    pub revision: u64,
    pub as_of_ms: i64,
    pub permitted: BTreeSet<String>,
    pub entities: BTreeMap<String, EntityV2Row>,
    pub aliases: BTreeMap<String, BTreeSet<String>>,
    pub focus: Vec<FocusRowV2>,
    pub observations: ValidatedObservations,
    pub notices: Vec<String>,
    pub seeds: Vec<String>,
    pub stale: bool,
    pub pending: bool,
}

#[path = "query_v2_activate.rs"]
mod activate;
#[path = "query_v2_context.rs"]
mod context;

pub use activate::activate_v2;
#[cfg(test)]
pub use activate::activate_v2_with_stats;
pub use context::load_query_context_v2;
