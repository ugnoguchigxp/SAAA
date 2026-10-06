//! Generic worker executor (docs/plans/worker-agents.md §5.2-5.5, §6.2).
//!
//! Admission binds a delegation to a persisted user input; the worker lane runs it through a tier
//! ladder with same-tier retries; every tool call is ledgered; the host (not the runner) checks
//! the finished output; asynchronous results reach the user only through the steward outbox.
//! Per-output-kind attempt logic sits behind `AttemptRunner`; this module owns everything generic.
mod admit;
mod attempt;
mod delivery;
#[allow(clippy::module_inception)]
mod executor;
mod json_runner;
mod ledger;
mod recover;
mod signals;
mod store;
mod verify;

#[cfg(test)]
mod test_fakes;
#[cfg(test)]
mod tests_admit;
#[cfg(test)]
mod tests_exec;
#[cfg(test)]
mod tests_ledger;
#[cfg(test)]
mod tests_recover;

use crate::worker_agents::contracts::AttemptRunner;
use std::sync::Arc;

pub(crate) use admit::admit;
pub(crate) use executor::Executor;
pub(crate) use json_runner::{compose_system, JsonRunner};
pub(crate) use ledger::LedgeredToolRunner;
pub(crate) use recover::{abandon_job, cancel_for_input, cancel_task, recover};

/// Attempt logic per output kind. `web_claims` is supplied by the web-search worker, `json` by
/// [`JsonRunner`] (or any other implementation).
pub(crate) struct Runners {
    pub web_claims: Arc<dyn AttemptRunner>,
    pub json: Arc<dyn AttemptRunner>,
}

/// Invoked with a conversation id after a committed transaction enqueued a steward outbox row.
pub(crate) type ReportHook = Arc<dyn Fn(&str) + Send + Sync>;
