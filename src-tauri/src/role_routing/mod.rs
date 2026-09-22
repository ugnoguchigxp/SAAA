//! Ownership, invariants, and code lookup: README.md in this directory.
//! Durable policy and ledger foundations for role-based model routing.

pub(crate) mod adapters;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod classifier;
pub(crate) mod context;
pub(crate) mod contracts;
pub(crate) mod coordinator;
pub(crate) mod driver;
pub(crate) mod executor;
pub(crate) mod ipc;
pub(crate) mod learning;
pub(crate) mod limits;
#[cfg(feature = "provider-diagnostics")]
pub mod operator_configuration;
pub(crate) mod proposals;
pub(crate) mod ranker;
pub(crate) mod recipe;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod recovery;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod reducer;
pub(crate) mod repository;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod review;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod revision;
pub(crate) mod schema;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod selection;
pub(crate) mod signals;
// The queue is an offline-gated contract until speech playback is wired to role roots.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod speech_queue;
pub(crate) mod speech_repository;
pub(crate) mod steps;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod tool_ledger;
pub(crate) mod tool_specialist;
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) mod tools;

pub(crate) use contracts::RoleRoutingSettings;

pub(crate) fn default_document_value() -> serde_json::Value {
    serde_json::to_value(RoleRoutingSettings::default())
        .expect("default role routing settings serialize")
}
