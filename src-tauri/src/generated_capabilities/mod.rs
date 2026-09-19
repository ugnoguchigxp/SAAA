//! Dynamically registered capabilities. The module keeps the trusted L-Lang runtime separate
//! from candidate packages, stores a persistent catalog in SQLite, and exposes
//! import/verify/activate/invoke/suspend behind a single service. Publication to conversation
//! tools lives in `publication`/`tools`; MCP and UI wiring arrive in later milestones.

pub mod contracts;
pub mod errors;
pub mod execution;
pub mod guards;
pub mod host;
pub mod lifecycle;
pub mod limits;
pub mod package_store;
pub mod publication;
pub mod recovery;
pub mod repository;
pub mod schema;
pub mod service;
pub mod tools;
pub mod verification;

#[cfg(test)]
pub(crate) mod tests;
