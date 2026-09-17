//! Durable Personal State owned by the existing SQLite writer.
mod codec;
pub mod commands;
pub mod contract;
pub mod generation;
#[cfg(test)]
pub mod inference;
pub mod jobs;
pub(crate) mod journal;
pub mod managed;
mod managed_tests;
mod materializer;
pub(crate) mod output;
pub mod projection;
#[cfg(test)]
mod registration;
mod scheduler;
pub mod schema;
pub mod sources;
pub mod store;
#[cfg(test)]
pub mod task_bundle;
mod tests;
pub mod worker;
mod worker_scope;
pub use codec::{decode, encode, now};
#[cfg(feature = "quality-eval-harness")]
pub mod live_harness;
pub mod product;
pub mod product_binding;
mod product_cleanup;
mod product_extract;
mod product_tests;
mod subject_pin;
