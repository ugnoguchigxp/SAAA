//! World Model adapter: typed payloads, projection index, bounded query and
//! the shared-commit validation hook. The canonical history stays in the
//! existing Personal State tables (WM-05..WM-13, D16..D37).

pub mod evidence_eligibility;
pub mod observations_v2;
#[allow(dead_code)]
pub mod outcome_v2;
pub mod projection;
pub mod projection_v2;
#[allow(dead_code)]
pub mod query;
#[allow(dead_code)]
pub mod query_v2;
pub mod runtime_capacity;
pub mod runtime_coding;
mod runtime_coding_tests;
pub mod runtime_frame;
mod runtime_frame_boundary_tests;
mod runtime_frame_perf_tests;
mod runtime_frame_snapshot_tests;
mod runtime_frame_tests;
pub mod runtime_graph;
pub mod runtime_scope;
mod runtime_scope_tests;
pub(crate) mod runtime_test_support;
pub mod test_support;
mod tests;
mod v2_tests;
pub mod validation;
pub mod validation_v2;
