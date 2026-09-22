//! Ownership, invariants, and code lookup: README.md in this directory.
pub(crate) mod commands;
pub(crate) mod contracts;
pub(crate) mod recovery;
pub(crate) mod repository;
// Delegated execution is retained behind its contract tests until the coordinator enables it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod service;
mod settings;
#[cfg(test)]
mod tests;
pub(crate) mod tools;
pub(crate) mod world_snapshot;
