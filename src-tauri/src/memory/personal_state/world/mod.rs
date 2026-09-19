//! World Model adapter: typed payloads, projection index, bounded query and
//! the shared-commit validation hook. The canonical history stays in the
//! existing Personal State tables (WM-05..WM-13).

pub mod projection;
// The bounded World query API is exercised by tests in M1; it is not yet wired
// to the Context Broker (see plan section 13).
#[allow(dead_code)]
pub mod query;
pub mod validation;

pub mod test_support;
mod tests;
