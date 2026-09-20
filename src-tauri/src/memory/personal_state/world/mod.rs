//! World Model adapter: typed payloads, projection index, bounded query and
//! the shared-commit validation hook. The canonical history stays in the
//! existing Personal State tables (WM-05..WM-13, D16..D37).

pub mod observations_v2;
#[allow(dead_code)]
pub mod outcome_v2;
pub mod projection;
pub mod projection_v2;
#[allow(dead_code)]
pub mod query;
#[allow(dead_code)]
pub mod query_v2;
pub mod test_support;
mod tests;
mod v2_tests;
pub mod validation;
pub mod validation_v2;
