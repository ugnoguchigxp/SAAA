//! Dynamically registered capabilities: import/verify/activate/invoke/suspend behind one service.

pub mod contracts;
pub mod errors;
pub mod execution;
pub mod generation;
pub mod guards;
pub mod host;
pub mod inspection;
pub mod lifecycle;
pub mod limits;
pub mod package_store;
pub mod publication;
pub mod publication_sync;
pub mod recovery;
pub mod repository;
pub mod retirement;
pub mod schema;
pub mod service;
pub mod tools;
pub mod verification;

#[cfg(test)]
pub(crate) mod tests;
