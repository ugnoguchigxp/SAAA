//! Bounded, side-effect-free World query (WM-09..WM-13).
//!
//! The reader never writes or repairs the projection. A stale or missing
//! projection is reported as an omission notice, not an error; authorization
//! failures are contract errors, never an empty slice.
//!
//! Every projected row is re-authorized and re-checked for current validity at
//! read time (R1/R2): the projection is a bounded index, not an authority.

use crate::database_error;
use rusqlite::{named_params, Connection};
use saaa_personal_state_core::world::model::{EntityKind, SliceEvidence, SliceFocus, SliceNode};
use saaa_personal_state_core::world::relevance::{
    build_gaps, focus_rank, node, relation as slice_relation, slice_path, trim_to_budget, GapInput,
    MAX_SLICE_BYTES,
};
use saaa_personal_state_core::world::traversal::{
    conditions_consistent, maximal_only, traverse, CausalDirection, EdgeStatus, Limits,
    TraversalMode, WorldEdge, WorldPath,
};
use saaa_personal_state_core::world::{normalize_name, WorldPayload, WorldSlice};
use saaa_personal_state_core::{AccessRequest, Classification, Purpose};
use std::collections::{BTreeMap, BTreeSet};
include!("query.d/01.rs");
include!("query.d/02.rs");
