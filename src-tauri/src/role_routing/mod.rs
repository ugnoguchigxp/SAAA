//! Durable policy and ledger foundations for role-based model routing.

pub(crate) mod adapters;
pub(crate) mod classifier;
pub(crate) mod contracts;
pub(crate) mod coordinator;
pub(crate) mod ipc;
pub(crate) mod learning;
pub(crate) mod limits;
pub(crate) mod ranker;
pub(crate) mod recovery;
pub(crate) mod reducer;
pub(crate) mod repository;
pub(crate) mod revision;
pub(crate) mod schema;
pub(crate) mod selection;
pub(crate) mod signals;
pub(crate) mod speech_queue;
pub(crate) mod tool_ledger;
pub(crate) mod tools;

pub(crate) use contracts::RoleRoutingSettings;

pub(crate) fn default_document_value() -> serde_json::Value {
    serde_json::to_value(RoleRoutingSettings::default())
        .expect("default role routing settings serialize")
}
