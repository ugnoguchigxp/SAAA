//! Worker agents: simple delegated tasks run by purpose-built agents with fixed tools.
//!
//! Plan and contracts: docs/plans/worker-agents.md. The conversation agent searches for an agent
//! (discovery) instead of searching for tools; the worker then uses its own pinned tool set.
pub(crate) mod contracts;
pub(crate) mod discovery;
pub(crate) mod executor;
pub(crate) mod loader;
pub(crate) mod registry;
pub(crate) mod schema;
pub(crate) mod web_search;

#[cfg(test)]
pub(crate) mod test_support;
