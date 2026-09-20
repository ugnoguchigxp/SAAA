#![allow(dead_code)]
//! Build one authorized graph query input and call the existing v2 reader
//! exactly once (R5). The request's access/now/request_id are reused verbatim;
//! no caller-supplied authorization is allowed inside. Condition/availability
//! observations and temporary attention are always empty in M2A.

use super::query::{WorldSeed, PENDING, STALE};
use super::query_v2::{activate_v2, ActivateInputV2};
use super::runtime_frame::{FrameRequest, GraphRequest};
use super::runtime_scope::AuthorizedFrame;
use crate::database_error;
use rusqlite::Connection;
use saaa_personal_state_core::world::normalize_name;
use saaa_personal_state_core::world::runtime_frame::{
    effective_max_bytes, FrameError, FrameNoticeCode, MAX_GRAPH_BYTES, MAX_TOTAL_NODES,
};
use saaa_personal_state_core::world::slice_v2::WorldSliceV2;
use std::collections::BTreeSet;

/// The M2A seed rule: deduplicate by the existing v2 identity (entity id or
/// normalized exact name) and allow at most 4 distinct seeds. A 5th distinct
/// seed is `frame-limit`, never silently truncated. An empty set is not an
/// error here; the caller returns an empty envelope without querying.
fn normalize_seeds(seeds: &[WorldSeed]) -> Result<Vec<WorldSeed>, FrameError> {
    let mut keys = BTreeSet::new();
    let mut unique = Vec::new();
    for seed in seeds {
        let key = match seed {
            WorldSeed::EntityId(id) => format!("id:{id}"),
            WorldSeed::ExactName(name) => format!("name:{}", normalize_name(name)),
        };
        if keys.insert(key) {
            unique.push(seed.clone());
        }
        if unique.len() > 4 {
            return Err(FrameError::Limit);
        }
    }
    Ok(unique)
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum GraphOutcome {
    Slice(WorldSliceV2),
    Notice(FrameNoticeCode),
}

fn map_query_error(error: String) -> FrameError {
    match error.as_str() {
        "world-scope-denied" => FrameError::ScopeDenied,
        "world-limit" => FrameError::Limit,
        _ => FrameError::Other(error),
    }
}

pub(crate) fn load_graph(
    c: &Connection,
    request: &FrameRequest<'_>,
    authorized: &AuthorizedFrame,
    now: i64,
    runtime_count: usize,
    graph_request: &GraphRequest,
) -> Result<GraphOutcome, FrameError> {
    let node_cap = MAX_TOTAL_NODES.saturating_sub(runtime_count);
    if node_cap == 0 {
        return Ok(GraphOutcome::Notice(FrameNoticeCode::WorldCapacityOmitted));
    }
    let mut limits = graph_request.limits.capped();
    limits.nodes = limits.nodes.min(node_cap);
    let seeds = normalize_seeds(&graph_request.seeds)?;
    // An empty seed set never fabricates a Project/Goal seed. Return an empty
    // envelope without calling the graph reader.
    if seeds.is_empty() {
        let revision: u64 = c
            .query_row(
                "SELECT revision FROM personal_scope WHERE id='primary'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| FrameError::Other(database_error(error)))?;
        return Ok(GraphOutcome::Slice(WorldSliceV2::empty(revision, now)));
    }
    // `activate_v2` rejects a budget below 256 bytes outright. A request whose
    // whole frame budget is that small still has to produce a frame (or a
    // frame-budget-too-small error) from the envelope, so the graph gets a
    // workable minimum and any overflow is dropped by the assembler.
    let max_bytes = effective_max_bytes(request.max_bytes).clamp(256, MAX_GRAPH_BYTES);
    let input = ActivateInputV2 {
        project_scope: &authorized.project_scope,
        access: &request.access,
        now,
        seeds: &seeds,
        causal_direction: graph_request.causal_direction,
        limits,
        max_bytes,
        request_id: &authorized.run_id,
        explicit_question: graph_request.explicit_question,
        flags: graph_request.flags,
        condition_observations: &[],
        availability_observations: &[],
        temporary_attention_entity_ids: &[],
    };
    let slice = match activate_v2(c, &input) {
        Ok(slice) => slice,
        // A budget too small for a graph envelope is an omission, not a frame
        // failure; corruption and authorization errors still propagate.
        Err(error) if error == "world-budget-too-small" => {
            return Ok(GraphOutcome::Notice(FrameNoticeCode::WorldCapacityOmitted));
        }
        Err(error) => return Err(map_query_error(error)),
    };
    if slice.notices.iter().any(|notice| notice == STALE) {
        return Ok(GraphOutcome::Notice(FrameNoticeCode::WorldProjectionStale));
    }
    if slice.notices.iter().any(|notice| notice == PENDING) {
        return Ok(GraphOutcome::Notice(FrameNoticeCode::WorldPending));
    }
    Ok(GraphOutcome::Slice(slice))
}
