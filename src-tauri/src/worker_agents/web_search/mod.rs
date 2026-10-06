//! Web search worker (docs/plans/worker-agents.md §6.3-6.6): the builtin profile, its attempt
//! runner, the injection checker, the claims validator and the source ledger.
pub(crate) mod audit;
pub(crate) mod checker;
pub(crate) mod claims;
pub(crate) mod envelope;
pub(crate) mod profile;
pub(crate) mod redact;
pub(crate) mod runner;
pub(crate) mod sources;

#[cfg(test)]
mod tests;

pub(crate) use audit::{task_stats, terminal_audit_attributes, TaskStats, TerminalAuditInput};
pub(crate) use profile::web_search_draft;
pub(crate) use runner::runner;
